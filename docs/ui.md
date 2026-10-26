# Player UI and debug overlay

Design for replacing the keyboard-and-text interface with an on-screen UI for players, while
keeping the current text and graph as a developer overlay. This is built; the notes at the end
say where it differs from the first plan. See [CLAUDE.md](../CLAUDE.md) for the
code layout and [sounds.md](sounds.md) for the sound list.

## Goals

- Everything a player needs can be done with the mouse or a finger, so the web build works on
  touch devices. Every keyboard shortcut keeps working.
- The screen shows only what a player needs. The statistics, percentile graph and key list become
  a debug overlay for development.
- Buttons and keys can't drift apart: both send the same action.

## Layout

```
┌────────────────────────────────────────────────────────────────────────┐
│ Low Bridge · goal 4 m        Gen 23 · best 2.85 m   3 stars until gen 30 │  top status bar
│                                                                        │
│                              [ 2.11 m ]                                │  distance readout (in world)
│                                ▣                  ▬▬▬▬     ⚑ GOAL       │
│▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│  ground
│ [⏭ Next gen][▶ Auto]  [‹][Best][Median][Worst][›]  [−] 4× [+][↻]  [🔊][☰] │  bottom button bar
└────────────────────────────────────────────────────────────────────────┘
```

The distance readout above the creature and the goal flag are part of the world, so they stay as
they are.

### Top status bar

- The level name and goal, or "Free play".
- The generation number and the best distance so far.
- In a level, what the next star needs, such as "3 stars until gen 30". It counts down to fewer
  stars as the generations pass, and reads "Goal reached in gen N" once the level is done.

### Bottom button bar

| Group | Buttons | Keys it replaces |
|---|---|---|
| Evolve | Next gen; Auto (stays highlighted while on) | Space, A |
| Watch | ‹ previous gen; Best, Median, Worst (the selected one is highlighted); next gen ›; a "Latest" chip while browsing back | 1/2/3, ←/→, End |
| Playback | − speed, the current speed, + speed; Replay | ↓/↑, Enter |
| System | Sound (cycles on → music off → muted); Menu; Restart | M, Esc, R |

- Each button's tooltip shows its keyboard shortcut.
- Zoom stays on the mouse wheel (and the −/= keys). It doesn't need buttons.
- The fine volume keys (`,` `.` `[` `]`) stay keyboard-only for now. Volume sliders belong in a
  settings panel on the menu later.
- Restart in a level starts a new attempt with a new seed, the same as Retry on the results
  screen.

## Debug overlay

The current text block (preset, percentile stats, status, label, sound line, key list) and the
percentile graph with its backdrop become the debug overlay.

- Off by default, on the web and natively.
- F3 toggles it at any time.
- `--debug` turns it on at startup natively; `?debug` in the page URL does the same on the web.
- While it's on, the player bars stay visible too. The debug text sits below the top bar.

## Icons

Bevy's built-in font has no glyphs for play, pause, arrows, stars or ×, which is why the current
HUD shows empty boxes. The bars use a bundled icon font (open-licensed, subset to the icons used,
in `assets/fonts/`). It also gives the results screen and menu real stars instead of dots.

## Implementation

- **`GameAction` message** (in `src/main.rs`): `NextGen`, `ToggleAuto`, `Watch(Which)`,
  `PrevGen`, `NextGenView`, `LatestGen`, `Faster`, `Slower`, `Replay`, `Restart`,
  `CycleSound`, `Menu`, `ToggleDebug`. Keyboard input becomes a small system that maps keys to
  actions, and buttons send the same actions. One system applies them. The menu and results
  screens keep their own actions in `levels.rs`.
- **`src/hud.rs`** (`HudPlugin`): spawns the top and bottom bars, writes `GameAction`s from button
  presses, and restyles buttons from game state (Auto on, which creature is watched, speed,
  sound). The bars only show in `Screen::Level` and `Screen::Results`.
- **`DebugUi` resource**: whether the debug overlay shows. `update_hud` and `draw_graph` check it.
- Buttons use `bevy::ui_widgets::Button` with `ActivateOnPress`, like the menu.
- The camera keeps the ground about 80% of the way down the screen, so the bottom bar sits over
  the ground. Check that the creature, its readout and the goal flag stay clear of both bars.

## Watching and auto-run

- A level starts evolving as soon as it opens: auto-run is on by default, and "Keep evolving" on
  the results screen turns it back on.
- The default view is the **leader**: the best creature from any generation so far. A
  generation's best can regress; the leader can't.
- While auto-running, a new leader takes over when the current replay finishes, so runs aren't cut
  off. Stepping by hand (auto off) switches straight away. The caption in the button bar always
  describes what's actually playing, which can lag the view by one replay.
- Browsing back (‹) switches from Leader to Best, since the leader isn't tied to one generation.

## Kick-off and pacing

Added after the first version, because auto-run finished levels before the player saw anything:

- **Fling to start.** A run waits for the player to grab the creature and throw it. The gesture
  is hashed into the seed, and evolution starts once the creature has tumbled for 2.5 s. A banner
  explains it; Space or the Next gen and Auto buttons skip it with a random seed.
- **Poking.** During a replay the creature can be grabbed and yanked. The run is marked nudged
  (grey readout, "(nudged)" caption) and can't trigger the finale.
- **Record pacing.** Auto-run pauses whenever a leader beats the last one shown by 10% and at least
  0.1 m, and resumes once its replay has played to the end. Measured on the four levels: 3 stops
  on First Steps, about 6 on the others, roughly 30–60 s of watching at 2×.
- **Finale.** Reaching the goal stops evolving and plays the winner; the results show as it
  crosses the goal flag on screen.
- **Record marker and progress.** A gold flag on the course marks the leader's distance, and the
  top bar has a progress bar toward the goal.

## How it was built

- Bars: the top bar shows during a level and on its results; the bottom bar only while the level
  is playing, since the results screen has its own buttons.
- Tooltips with keyboard shortcuts were dropped for now: Bevy 0.20 has no tooltip widget. The
  shortcuts are in the README and the debug overlay's key list.
- Icons: Inter, already open-licensed and on hand, has ▶ ⇥ ‹ › ↻ ↺ ★ ☆ and −, so it replaces Bevy's
  default font everywhere (built into the binary from `assets/fonts/`) instead of a separate icon
  font. It has no pause or speaker symbols, so Auto shows its state by highlighting and the sound
  button uses words.
- Keys added: L watches the leader, F3 toggles the debug overlay.

## Later

- Tooltips showing each button's shortcut.

- A settings panel on the menu: volume sliders, debug toggle, playback defaults.
- Pinch to zoom on touch screens.
- Saving settings with the rest of the planned persistence.
