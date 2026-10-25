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
  a dead-zone, critically damped camera (`CameraRig`, `smooth_damp`).
- `web/index.html`: the page that loads the wasm build and focuses Bevy's canvas.
- `.github/workflows/pages.yml`: builds wasm and deploys to GitHub Pages on every push to `main`.

## Commands

    cargo run --release                     # native app; see README for flags
    cargo test --release                    # sim tests (evolves forward, stays compact, topology mutates)
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
- When testing in an embedded or hidden browser, frame-rate numbers are throttled to about 1 Hz.
  Check `longtask` entries instead of FPS to tell whether the page itself is busy.

## Where this is heading: gamification

The plan is to make it a game while trying more of Bevy. The first slice:

1. **Obstacle courses.** A level-select menu in bevy_ui, using States (Menu → Level → Results),
   with 3–4 courses in `assets/levels/*.ron` (ground, obstacles, goal distance), hot-reloaded
   while designing them. Reaching the goal shows a results screen with stars based on how many
   generations it took.
2. **Ghost races.** The last few champions replay as translucent ghosts beside the current run.

Later ideas, each aimed at a different Bevy feature:

- **Hand breeding**: pick parents from a 3×3 grid of live previews, using render-to-texture
  cameras shown in UI buttons.
- **Creature editor**: drag nodes and draw muscles with bevy_picking and observers, then evolve
  the design.
- **Effects and feedback**: bevy_audio clicks when nodes land, a custom Material2d shader tinting
  muscles by tension, particles and screen shake on records, slow-motion via `Time<Virtual>`.
- **Persistence**: unlocked levels and best scores, in files natively and `localStorage` on the
  web.

Check Bevy 0.20's actual APIs in the crate source (`~/.cargo/registry/src/*/bevy_*-0.20*`)
before writing against them; Bevy changes a lot between releases.
