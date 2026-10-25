//! Sound: effects synthesized from what the playback does, a procedural music loop, and separate
//! music and effects volumes. Nothing is loaded from files, so the web download doesn't grow.
//!
//! Gameplay code only writes messages (`Landed`, `MetrePassed`, `NewRecord`); this plugin
//! decides what, if anything, to play.

use bevy::audio::{AddAudioSource, ChannelCount, Source, Volume};
use bevy::prelude::*;
use std::f32::consts::TAU;
use std::time::Duration;

const SAMPLE_RATE: u32 = 44_100;
/// Most effects playing at once; anything past this is dropped.
const MAX_VOICES: usize = 12;
/// Most landing thuds started in one frame (the loudest win).
const THUDS_PER_FRAME: usize = 2;
/// Shortest gap between record fanfares, in seconds, so auto-run doesn't spam them.
const FANFARE_GAP: f32 = 1.5;

/// A node hit the ground. `speed` is its downward speed (units per step) just before impact.
#[derive(Message)]
pub struct Landed {
    pub speed: f32,
    /// Ground friction, 0..1. Grippy black anchor feet sound deeper.
    pub friction: f32,
}

/// The watched creature passed this metre post for the first time this run.
#[derive(Message)]
pub struct MetrePassed(pub i32);

/// A generation beat the best distance so far.
#[derive(Message)]
pub struct NewRecord;

#[derive(Resource)]
pub struct AudioSettings {
    pub muted: bool,
    /// Music and effects volumes, 0..1.
    pub music: f32,
    pub sfx: f32,
    /// Browsers block audio until the first key press or click. Effects are dropped until then,
    /// rather than queuing up and all playing at once.
    pub unlocked: bool,
}

impl AudioSettings {
    fn music_volume(&self) -> Volume {
        Volume::Linear(if self.muted { 0.0 } else { self.music * self.music })
    }
    fn sfx_gain(&self) -> f32 {
        if self.muted { 0.0 } else { self.sfx * self.sfx }
    }
}

pub struct SoundPlugin;

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<Synth>()
            .add_message::<Landed>()
            .add_message::<MetrePassed>()
            .add_message::<NewRecord>()
            .insert_resource(AudioSettings {
                muted: false,
                music: 0.5,
                sfx: 0.8,
                unlocked: !cfg!(target_arch = "wasm32"),
            })
            .init_resource::<SfxState>()
            .add_systems(Update, (unlock, volume_keys, start_music, apply_music_volume, play_sfx).chain());
    }
}

#[derive(Component)]
struct MusicTrack;
#[derive(Component)]
struct SfxVoice;

#[derive(Resource, Default)]
struct SfxState {
    /// For detuning; separate from the simulation's RNG so sound never affects evolution.
    rng: u32,
    last_fanfare: f32,
}

impl SfxState {
    /// Uniform in [-1, 1).
    fn jitter(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

fn unlock(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    mut settings: ResMut<AudioSettings>,
) {
    if !settings.unlocked
        && (keys.get_just_pressed().next().is_some()
            || mouse.get_just_pressed().next().is_some()
            || touches.any_just_pressed())
    {
        settings.unlocked = true;
    }
}

fn volume_keys(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<AudioSettings>) {
    let step = |v: f32, up: bool| ((v + if up { 0.1 } else { -0.1 }) * 10.0).round().clamp(0.0, 10.0) / 10.0;
    if keys.just_pressed(KeyCode::KeyM) {
        settings.muted = !settings.muted;
    }
    for (key, up) in [(KeyCode::Comma, false), (KeyCode::Period, true)] {
        if keys.just_pressed(key) {
            settings.music = step(settings.music, up);
        }
    }
    for (key, up) in [(KeyCode::BracketLeft, false), (KeyCode::BracketRight, true)] {
        if keys.just_pressed(key) {
            settings.sfx = step(settings.sfx, up);
        }
    }
}

fn start_music(
    mut commands: Commands,
    settings: Res<AudioSettings>,
    mut synths: ResMut<Assets<Synth>>,
    playing: Query<(), With<MusicTrack>>,
) {
    if settings.unlocked && playing.is_empty() {
        commands.spawn((
            AudioPlayer(synths.add(Synth::Music)),
            PlaybackSettings::LOOP.with_volume(settings.music_volume()),
            MusicTrack,
        ));
    }
}

/// Playing audio ignores `PlaybackSettings` changes, so the music bus goes through its sink.
fn apply_music_volume(settings: Res<AudioSettings>, mut sinks: Query<&mut AudioSink, With<MusicTrack>>) {
    if settings.is_changed() {
        for mut sink in &mut sinks {
            sink.set_volume(settings.music_volume());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn play_sfx(
    mut commands: Commands,
    settings: Res<AudioSettings>,
    time: Res<Time>,
    mut state: ResMut<SfxState>,
    mut synths: ResMut<Assets<Synth>>,
    voices: Query<(), With<SfxVoice>>,
    mut landed: MessageReader<Landed>,
    mut metres: MessageReader<MetrePassed>,
    mut records: MessageReader<NewRecord>,
) {
    let gain = settings.sfx_gain();
    let mut free = MAX_VOICES.saturating_sub(voices.iter().len());
    if !settings.unlocked || gain <= 0.0 {
        free = 0;
    }
    let mut play = |synth: Synth, volume: f32, speed: f32| {
        if free == 0 {
            return;
        }
        free -= 1;
        commands.spawn((
            AudioPlayer(synths.add(synth)),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume * gain)).with_speed(speed),
            SfxVoice,
        ));
    };

    // Milestones first, so a pile of thuds can't take their voices.
    let now = time.elapsed_secs();
    if records.read().count() > 0 && now - state.last_fanfare > FANFARE_GAP {
        state.last_fanfare = now;
        play(Synth::Fanfare, 0.4, 1.0);
    }
    if let Some(MetrePassed(m)) = metres.read().last() {
        play(Synth::Ding { freq: metre_pitch(*m) }, 0.3, 1.0);
    }

    let mut hits: Vec<&Landed> = landed.read().collect();
    hits.sort_by(|a, b| b.speed.total_cmp(&a.speed));
    for hit in hits.into_iter().take(THUDS_PER_FRAME) {
        let loudness = ((hit.speed - 0.01) / 0.06).clamp(0.08, 1.0);
        let freq = 120.0 - 50.0 * hit.friction;
        let detune = 1.0 + 0.06 * state.jitter();
        // Two full-strength thuds plus a ding still stay under clipping at 100% effects volume.
        play(Synth::Thud { freq }, 0.5 * loudness, detune);
    }
}

/// Each metre climbs a pentatonic scale, so a good run plays a little tune.
fn metre_pitch(m: i32) -> f32 {
    const PENTATONIC: [i32; 5] = [0, 2, 4, 7, 9];
    let m = m.max(1) - 1;
    let note = (72 + PENTATONIC[(m % 5) as usize] + 12 * (m / 5)).min(100);
    midi_hz(note as f32)
}

fn midi_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

// ------------------------------------------------------------------------------------------------
// Synthesis
// ------------------------------------------------------------------------------------------------

/// A sound generated on the fly. Each one-shot gets its own tiny asset, freed when it finishes.
#[derive(Asset, TypePath, Clone, Copy)]
enum Synth {
    /// A low thump that drops in pitch, with a click of noise on impact.
    Thud { freq: f32 },
    /// A bell: two inharmonic partials with a long decay.
    Ding { freq: f32 },
    /// A rising arpeggio.
    Fanfare,
    /// An endless, gentle loop of plucked arpeggios over a bass line.
    Music,
}

impl Synth {
    /// Length in seconds, or `None` for the music, which never ends.
    fn length(self) -> Option<f32> {
        match self {
            Synth::Thud { .. } => Some(0.25),
            Synth::Ding { .. } => Some(1.4),
            Synth::Fanfare => Some(1.0),
            Synth::Music => None,
        }
    }
}

impl Decodable for Synth {
    type Decoder = SynthDecoder;

    fn decoder(&self) -> SynthDecoder {
        SynthDecoder {
            synth: *self,
            n: 0,
            end: self.length().map(|s| (s * SAMPLE_RATE as f32) as u64),
            phase: 0.0,
            noise: 0x1234_5678,
        }
    }
}

pub struct SynthDecoder {
    synth: Synth,
    /// Samples played so far. The music runs for hours, so time is worked out from this in f64.
    n: u64,
    end: Option<u64>,
    /// Running oscillator phase in cycles, for the thud's pitch sweep.
    phase: f32,
    noise: u32,
}

impl SynthDecoder {
    fn white(&mut self) -> f32 {
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        self.noise as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// A short attack, so notes don't click on.
fn attack(t: f32) -> f32 {
    (t * 400.0).min(1.0)
}

/// A plucked note `t` seconds after it started.
fn pluck(freq: f32, t: f32, decay: f32) -> f32 {
    let x = TAU * freq * t;
    attack(t) * (-t * decay).exp() * (x.sin() + 0.3 * (2.0 * x).sin() * (-t * decay * 2.0).exp())
}

impl Iterator for SynthDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.end.is_some_and(|end| self.n >= end) {
            return None;
        }
        let t_long = self.n as f64 / SAMPLE_RATE as f64;
        let t = t_long as f32;
        self.n += 1;
        let s = match self.synth {
            Synth::Thud { freq } => {
                let f = freq * (1.0 + 1.5 * (-t * 40.0).exp());
                self.phase = (self.phase + f / SAMPLE_RATE as f32).fract();
                let body = (TAU * self.phase).sin() * (-t * 18.0).exp();
                let click = self.white() * (-t * 250.0).exp() * 0.35;
                attack(t) * body + click
            }
            Synth::Ding { freq } => {
                let env = attack(t) * (-t * 3.5).exp();
                env * ((TAU * freq * t).sin() + 0.4 * (TAU * freq * 2.76 * t).sin() * (-t * 6.0).exp())
            }
            Synth::Fanfare => {
                const NOTES: [f32; 4] = [72.0, 76.0, 79.0, 84.0];
                const GAP: f32 = 0.09;
                let mut s = 0.0;
                for (i, note) in NOTES.iter().enumerate() {
                    let start = i as f32 * GAP;
                    if t >= start {
                        let decay = if i == NOTES.len() - 1 { 4.0 } else { 9.0 };
                        s += pluck(midi_hz(*note), t - start, decay);
                    }
                }
                s * 0.5
            }
            Synth::Music => music(t_long),
        };
        Some(s * 0.5)
    }
}

impl Source for SynthDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        ChannelCount::new(1).unwrap()
    }
    fn sample_rate(&self) -> bevy::audio::SampleRate {
        bevy::audio::SampleRate::new(SAMPLE_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.synth.length().map(Duration::from_secs_f32)
    }
}

/// The music at `t` seconds: Am–F–C–G at 96 bpm, an eighth-note arpeggio over a bass on the beat.
fn music(t: f64) -> f32 {
    const EIGHTH: f64 = 60.0 / 96.0 / 2.0;
    const BASS: [f32; 4] = [45.0, 41.0, 48.0, 43.0];
    const CHORDS: [[f32; 4]; 4] = [
        [57.0, 60.0, 64.0, 69.0],
        [53.0, 57.0, 60.0, 65.0],
        [55.0, 60.0, 64.0, 67.0],
        [55.0, 59.0, 62.0, 67.0],
    ];
    // Indices into the chord; -1 is a rest. The second half of the loop varies the first.
    const PATTERN: [[i32; 8]; 2] = [[0, 2, 1, 3, -1, 2, 3, 1], [0, 1, 2, 3, 2, -1, 1, 2]];

    let step = (t / EIGHTH) as i64;
    let mut s = 0.0;
    // The current note plus the tails of the previous ones, which are still ringing.
    for back in 0..3 {
        let k = step - back;
        if k < 0 {
            break;
        }
        let since = (t - k as f64 * EIGHTH) as f32;
        let bar = (k / 8) as usize;
        let chord = bar % 4;
        let pick = PATTERN[(bar / 4) % 2][(k % 8) as usize];
        if pick >= 0 {
            s += 0.12 * pluck(midi_hz(CHORDS[chord][pick as usize]), since, 7.0);
        }
        if k % 2 == 0 {
            s += 0.25 * pluck(midi_hz(BASS[chord]), since, 3.0);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synths_stay_in_range_and_one_shots_end() {
        let sounds = [Synth::Thud { freq: 70.0 }, Synth::Thud { freq: 120.0 }, Synth::Ding { freq: metre_pitch(30) }, Synth::Fanfare];
        for synth in sounds {
            let samples: Vec<f32> = synth.decoder().collect();
            assert_eq!(samples.len(), (synth.length().unwrap() * SAMPLE_RATE as f32) as usize);
            assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        }
        // An hour into the music it should still be clean and in range.
        let mut music = Synth::Music.decoder();
        music.n = 3600 * SAMPLE_RATE as u64;
        assert!(music.take(SAMPLE_RATE as usize * 20).all(|s| s.is_finite() && s.abs() <= 1.0));
    }
}
