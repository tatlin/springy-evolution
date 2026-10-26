# springy-evolution

A Rust + [Bevy 0.20](https://bevyengine.org) rebuild of
[tatlin/evolution3WEB](https://github.com/tatlin/evolution3WEB), which is itself a tweak of
Cary Huang's Processing sketch *Evolution Simulator* (openprocessing #205807).

Creatures are nodes (circles with ground friction) joined by muscles (clamped springs that switch
between a contracted and an extended rest length on a per-creature heartbeat). Each generation,
every creature gets 15 simulated seconds to travel as far right as it can. Survivors are picked by
the original's rank-pairing lottery, then each survivor is cloned once and has one mutated child.

## Run

The game opens on a level-select menu: four obstacle courses, plus free play. The run flags below
only affect free play; levels always use the same settings so their stars are comparable.

    cargo run --release                 # evolution3WEB settings (default)
    cargo run --release -- --cary       # Cary Huang's original settings
    cargo run --release -- --seed 38 --pop 500
    cargo run --release -- --max-len 6     # allow longer muscles (default 2.0)
    cargo run --release -- --fixed-topology  # never add/remove nodes or muscles
    cargo run --release -- --min-connections 0  # sparse regrowth, as in evolution3WEB
    cargo run --release -- --debug      # start with the developer overlay on (F3 toggles it)

On the web, add `?debug` to the page URL for the same overlay.

## Web

Play it in the browser at <https://tatlin.github.io/springy-evolution/> (click the page first so
it gets keyboard focus). Every push to `main` builds it for `wasm32-unknown-unknown` and deploys
it with `.github/workflows/pages.yml`. wasm has no threads, so the web build tests creatures on the
main thread, about 10 ms per frame, instead of on a worker. To build it locally (the
`wasm-bindgen` CLI must match the version in `Cargo.lock`):

    rustup target add wasm32-unknown-unknown
    cargo build --profile web --target wasm32-unknown-unknown
    wasm-bindgen --target web --no-typescript --out-dir dist \
      target/wasm32-unknown-unknown/web/springy-evolution.wasm
    cp web/index.html dist/ && cp -r assets dist/ && python3 -m http.server -d dist

## Levels

Each course is a file in `assets/levels/`: a goal distance, obstacles, and how many generations
earn three and two stars. When a generation's best creature reaches the goal, evolution stops and
a results screen shows your stars over the winning run. From there you can keep evolving, retry
with a new seed, go on to the next level, or return to the menu. The menu shows your best stars
this session (they aren't saved yet).

| Level | Course | Goal |
|---|---|---|
| 1. First Steps | flat ground | 3 m |
| 2. Low Bridge | a roof 25 cm off the ground from 1 m to 3 m | 4 m |
| 3. Staircase | three 4 cm steps up | 4 m |
| 4. Speed Bumps | three 4 cm bumps | 4 m |

Levels use the evolution3WEB settings with a population of 500. The star thresholds were set by
evolving each course from 10 seeds: three stars covers about the best 40% of seeds, two stars
about 80%. A level file looks like this (distances in metres, y up):

    (
        name: "Low Bridge",
        blurb: "Mind your head: a low roof from 1 m to 3 m.",
        goal: 4.0,
        stars: (three: 30, two: 40),
        obstacles: [
            (x: 1.0, width: 2.0, height: 1.0, lift: 0.25),  // lift raises it off the ground
        ],
        // population: 500,  // optional
    )

Natively, saving a level file while you play it restarts the level with the new layout. To add a
level, add its file to `LEVEL_FILES` in `src/levels.rs` (the browser can't list a folder).

## Controls

A run starts with a fling: grab the creature with the mouse or a finger and throw it. The gesture
becomes the run's random seed (shown in the top bar), and once the creature has tumbled for a
moment, evolution starts on its own. Space skips the fling and picks a random seed. Restart and
Retry ask for a new fling.

You watch the leader: the best creature from any generation so far. Whenever evolution finds a
leader that beats the last one shown by at least 10% (and 0.1 m), it pauses and plays that run in
full, so the game moves through quiet stretches quickly and stops for real progress. A gold flag
on the course marks the best distance so far. In a level, the results screen appears when you see
the winning creature cross the goal flag. Levels start at 2× playback speed.

You can also grab the creature during any replay and yank it around. That doesn't affect
evolution; the run is marked "nudged" and its distance readout turns grey. A status bar along the top shows the level, the generation, the best distance and
what the next star needs. A button bar along the bottom covers everything below except zoom and
the fine volume keys, so the game also works with a mouse or a touch screen.

| Key | Action |
|---|---|
| 1–4, F | menu: start a level, or free play |
| Esc | back to the level select |
| Space | simulate one generation (queues if pressed repeatedly) |
| A | toggle auto-run (on by default) |
| L | watch the leader (the default) |
| 1 / 2 / 3 | watch the latest generation's best / median / worst creature |
| ← / → / End | browse earlier generations / back to latest |
| ↑ / ↓ | playback speed (1× … 64×, or instant) |
| Enter | replay |
| Mouse wheel, − / = | zoom |
| P | switch preset and restart (free play) |
| R | restart with a new seed |
| M | mute / unmute |
| , / . | music volume down / up |
| [ / ] | effects volume down / up |
| F3 | developer overlay: stats, key list, percentile graph |

The sound button cycles between sound on, music off and muted. On the results screen: Space keeps evolving, Enter retries, N goes to the next level, and Esc
returns to the menu. Menu and results buttons can also be clicked.

## Sound

Everything you hear is synthesized in code (`src/audio.rs`), so the web build downloads no
audio files. Nodes thud when they land, louder the harder they hit and deeper for grippy black
feet. Each new metre post rings a bell that climbs a pentatonic scale, and a generation that beats
the best distance so far plays a short fanfare. Finishing a level plays a longer one that lands
on a ringing chord.

Going backwards has its own, quieter sounds. Posts behind the start ring a bell that descends a
minor scale. Falling back behind the furthest post plays a downward "bwomp", and a run that ends
behind the start line gets a sad trombone. A generation whose best is more than 5% worse than the
previous generation's plays a soft deflating "wah-wah". Each of these, and the fanfare, is
rate-limited so auto-run and rocking creatures don't repeat them. Landing sounds stop above 4×
playback, and the progress sounds stop in instant playback. A gentle procedural loop plays
underneath as music. In the browser, sound starts after your first key press or click.

[docs/sounds.md](docs/sounds.md) lists every sound the game uses or will need, with specs for
replacing the synthesized ones with designed audio.

## Layout

- `src/sim.rs` — the whole simulation and genetic algorithm, no Bevy. A faithful port of
  `simulate()`, `Muscle.applyForce()`, `Node.hitWalls()`, `toStableConfiguration()`,
  `modified()`/`modifyNode()`/`modifyMuscle()`, and the kill/reproduce stages. Deterministic per
  seed; evaluation is spread across all cores with `std::thread::scope`.
- `src/main.rs` — Bevy front end. Generations run on a worker thread so the window never stalls.
  Playback follows real time at any frame rate (60 sim steps per second at 1×).
- `src/audio.rs` — synthesized sound effects, procedural music, and the volume controls.
- `src/levels.rs` — level files, the menu and results screens, and stars.
- `src/hud.rs` — the in-level status bar and button bar.
- `src/fling.rs` — grabbing the creature: the kick-off fling and poking during replays.
- `assets/levels/` — the courses.
- `assets/fonts/` — Inter (SIL Open Font License), built into the binary as the UI font.
- `docs/` — design notes: the [player UI](docs/ui.md) and the [sound list](docs/sounds.md).

## The two presets

**evolution3WEB** (default) reproduces tatlin's fork as written:

- Generation 1 is a 2×2 "MDJ_Truss": four nodes braced with diagonals, every member permanently
  contracted (contract time = extend time), so the first generation can't move at all.
- The muscle-mutation loop in `modified()` is commented out, so a mutated child starts with no
  muscles and `checkForLoneNodes()` regrows one random muscle per node. Real locomotion only
  appears from generation 2 onward, and it comes from those regrown muscles.
- Two hurdles at 0.4–0.6 m and 0.8–1.0 m.

**Cary Huang** restores the original: 4–7 random nodes, all muscles inherited and mutated,
nodes/muscles added and removed by mutation, overlapping muscles pruned, flat ground.

## Deliberate differences from the Processing code

- The hurdles in evolution3WEB are declared with y1 > y2, which makes the original collision
  test always fail (they're drawn but nothing ever hits them). Rectangles are normalized here, so
  they're solid.
- A node centred exactly on an obstacle edge divides by zero in the original and the creature
  becomes NaN. That case now pushes the node out along the wall normal; creatures that still blow
  up score −∞ instead of corrupting the sort.
- `checkForOverlap()` can remove extra muscles in the original when a muscle is flagged twice;
  each muscle is removed at most once here.
- The RNG is splitmix64, so seeds won't reproduce the Processing runs.
- Newly grown muscles are capped at `max_muscle_length` (2.0, the same limit the original's
  `modifyMuscle()` clamps to). In evolution3WEB a regrown muscle copies whatever distance its
  nodes have drifted to, so without the cap creatures sprawl to 100+ units across after a few
  hundred generations, and that sprawl is how they score 15–20 m. With the cap they stay
  compact, but the evolution3WEB preset plateaus near 1 m; `--max-len` trades one for the other.
- Topology mutation (adding/removing nodes and muscles) runs in both presets. In evolution3WEB
  it was skipped along with the muscle-mutation loop, so every creature stayed at four nodes.
  `--fixed-topology` turns it off.
- In evolution3WEB, `checkForLoneNodes()` only regrows enough muscles that no node has fewer
  than two, so creatures end up as floppy chains. Here each node is braced to at least 3
  others (`min_connections`), farthest first, so opposite corners get cross-braced and a
  4-node creature gets back the truss's full bracing. Cary Huang's preset keeps 0.
  `--min-connections` sets it for either preset.

## License

The original is CC BY-SA 3.0 / GPL 2.0 (Cary Huang); this port is GPL-2.0-or-later.
