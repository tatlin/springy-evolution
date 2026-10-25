# CLAUDE.md

Notes for Claude Code sessions on this repo. The user-facing docs are in [README.md](README.md):
run flags, controls, the two presets, and every deliberate difference from the Processing original.
Read it first; this file only adds what a contributor needs on top.

## What this is

A Rust + Bevy 0.20 port of Cary Huang's Evolution Simulator (via tatlin/evolution3WEB): creatures
of nodes and muscles evolve to travel right in 15 simulated seconds. It runs natively and in the
browser at <https://tatlin.github.io/springy-evolution/>.

## Layout

- `src/sim.rs`: the simulation and genetic algorithm, with no Bevy. Deterministic per seed.
  `Config` holds every knob, `Population::step` runs one generation, and
  `evaluate_batch` + `finish_generation` split a generation so it can be spread across frames.
- `src/main.rs`: the Bevy front end. It plays back one creature (`Playback`), runs generations
  via `Worker`, draws with a pooled set of node and muscle entities, and follows the creature with
  a dead-zone, critically damped camera (`CameraRig`, `smooth_damp`). `step_playback` owes sim
  steps from `Time` (`STEP_RATE` = 60/s at 1×), so playback speed doesn't depend on frame rate,
  and it writes the audio messages as it steps. A run is built from a `Setup` (free play's preset
  and CLI overrides, or a `LevelRun`): `start_run` replaces `Evo`, and `enter_level` builds the
  setup for a level index or free play. `poll_worker` detects a level's goal, sets
  `Evo::finished_at`, and switches to `Screen::Results`.
- `src/levels.rs`: `LevelsPlugin`. The `Level` asset and its RON loader (`*.level.ron`), the
  `Screen` state (Menu → Level → Results), the menu and results UI, keyboard shortcuts, session
  stars (`Progress`), and hot reload of the level being played. `input` and `poll_worker` only
  run in `Screen::Level`, so the menu sits over a creature that keeps playing.
- `assets/levels/`: the courses. `LEVEL_FILES` lists them, because the web can't list folders.
- `src/audio.rs`: `SoundPlugin`. Gameplay writes `Landed` (one per node landing) and `Milestone`
  messages (`Metre(i32)`, with negative posts behind the start, plus `SlippedBack`, `EndedBehind`,
  `NewRecord`, `Regressed` and `GoalReached`), and the plugin decides what to play and rate-limits repeats.
  Thresholds that decide *whether* something happened (`SLIP_SLACK`, `REGRESSION_DROP`) live
  in `main.rs`, and the repeat gaps live in `audio.rs`. Add new progress sounds as `Milestone`
  variants rather than new message types, so systems stay under Bevy's argument limit. All
  sound is a `Synth` asset (a custom `Decodable`) generated in code, one tiny asset per one-shot,
  plus an endless procedural music loop. `AudioSettings` holds mute, the music and effects
  volumes, and `unlocked`.
- `docs/sounds.md`: the sound-design list (what plays now, what's planned, specs for a designer).
  Update its status column when you add or replace a sound.
- `web/index.html`: the page that loads the wasm build, focuses Bevy's canvas, and resumes the
  AudioContext on the first input.
- `.github/workflows/pages.yml`: builds wasm and deploys to GitHub Pages on every push to `main`.

## Commands

    cargo run --release                     # native app; see README for flags
    cargo test --release                    # sim, synth and level-file tests
    cargo build --profile web --target wasm32-unknown-unknown   # web build (README "Web" has the rest)

The Rust version is pinned to 1.99.1 in `mise.toml`. rustfmt isn't enforced; the code uses wide
single-line `if`/`let-else` style, so match the surrounding code rather than reformatting files.

## Native vs web

wasm32-unknown-unknown has no threads and no `SystemTime`, so these spots are split with
`#[cfg(target_arch = "wasm32")]`; keep both paths working when you touch them:

- `Worker`: natively a background thread runs whole generations. On the web the app evaluates
  creatures for 10 ms per frame (`Worker::BUDGET`).
- `Population::evaluate_all`: natively it uses `std::thread::scope` across all cores. On the
  web it runs single-threaded.
- `random_seed`: `SystemTime` natively, `js_sys::Math::random` on the web.

The web build is about 96 MB raw and 16 MB gzipped. Audio can only start after the first user
gesture, and assets load over HTTP, so anything in an `assets/` folder must be copied into `dist/`
by the workflow.

## Gotchas we've hit

- The `wasm-bindgen` CLI must exactly match the `wasm-bindgen` version in `Cargo.lock`. The
  workflow reads it from the lockfile; locally use
  `cargo install wasm-bindgen-cli --version <x> --locked`.
- The `github-pages` environment only allows deploys from `main`. Enabling Pages, editing that
  environment, or cancelling runs needs a `gh` account with admin rights on the repo.
- In the evolution3WEB preset, regrown muscles copy their nodes' current distance as their rest
  length, so without `max_muscle_length` creatures sprawl to 100+ units. Fitness tracks size
  closely there: a longer `--max-len` gives better scores but bigger creatures.
- A black node is maximum ground friction, not a bug. Evolution uses one as an anchor foot.
- Browsers create the AudioContext suspended, and cpal only calls `resume()` once, at startup, so
  without help the web build is silent forever. `web/index.html` wraps `window.AudioContext` in a
  classic `<script>` and resumes every context on keydown/pointerdown/touchend. It must stay a
  classic script before the module, because the wasm-bindgen JS captures `AudioContext` when the
  module is evaluated. Rust drops effects until `AudioSettings::unlocked` so they don't pile up
  while the context is suspended.
- `GlobalVolume` and `PlaybackSettings` don't affect audio that's already playing. Change the music
  volume through its `AudioSink`, as `apply_music_volume` does.
- Browser automation sends punctuation keys without `KeyboardEvent.code`, so winit ignores them.
  To test `,` `.` `[` `]` in the web build, dispatch real `KeyboardEvent`s on the canvas.
  To check that sound is playing, wrap `AudioBuffer.prototype.copyToChannel` and look at the
  sample peaks.
- When testing in an embedded or hidden browser, frame-rate numbers are throttled to about 1 Hz.
  Check `longtask` entries instead of FPS to tell whether the page itself is busy. A throttled
  page also evolves very slowly (about a generation every 5 s), so check `document.visibilityState`
  before deciding the game is stuck.
- `bevy::ui_widgets::Button` activates on release only if its `Pressed` marker is still there at
  click time. A press and release in the same frame (browser automation, fast taps) misses it, so
  our buttons use `ActivateOnPress`.
- A local variable named `setup` in `main()` shadows the `setup` system, and the error is an
  unhelpful "does not describe a valid system configuration".
- Level difficulty comes almost entirely from obstacles. On flat ground creatures reach 1 m in about
  2 generations and 12–18 m in 120, but a single 12 cm wall takes about 85 generations. After
  changing a course or the sim, re-tune its stars by evolving it headless from about 10 seeds with
  `springy_evolution::sim` (the library has no Bevy) and taking the 40th and 80th percentiles.

## Where this is heading: gamification

The plan is to make it a game while trying more of Bevy. The first slice:

1. **Obstacle courses.** Done: a level-select menu in bevy_ui, using States (Menu → Level →
   Results), with four courses in `assets/levels/*.level.ron`, hot-reloaded while designing them.
   Reaching the goal shows a results screen with stars based on how many generations it took.
   Not yet: saving stars, unlocking levels in order, terrain other than rectangles (slopes, pits),
   and level sounds beyond the goal fanfare.
2. **Ghost races.** The last few champions replay as translucent ghosts beside the current run.

Later ideas, each aimed at a different Bevy feature:

- **Hand breeding**: pick parents from a 3×3 grid of live previews, using render-to-texture
  cameras shown in UI buttons.
- **Creature editor**: drag nodes and draw muscles with bevy_picking and observers, then evolve
  the design.
- **Effects and feedback**: landing thuds, metre bells, the record and goal fanfares, and the
  going-backwards sounds are in. Still to do: a custom Material2d shader tinting muscles by tension, particles and screen shake on records, slow-motion via `Time<Virtual>`.
- **Persistence**: unlocked levels and best scores, in files natively and `localStorage` on the
  web.

Check Bevy 0.20's actual APIs in the crate source (`~/.cargo/registry/src/*/bevy_*-0.20*`)
before writing against them; Bevy changes a lot between releases.
