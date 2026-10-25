# Sound design list

Every sound the game uses now or will need for the roadmap in [CLAUDE.md](../CLAUDE.md), for
briefing a sound designer. Everything playing today is synthesized in code in
[`src/audio.rs`](../src/audio.rs). Each designed sound can replace its synthesized one, which
stays as the fallback until then.

**Status:** *Synth* means a synthesized placeholder is playing in the game now. *Planned* means
nothing plays yet.

## Specs for every sound

- **Format:** mono `.ogg` (Bevy's default build only decodes Vorbis), 44.1 kHz.
- **Level:** normalize to about −3 dBFS peak. Up to 12 effects can play at once, so leave
  headroom. The going-backwards sounds should sit noticeably quieter than the positive ones.
- **Variations:** anything that fires often needs 3–5 takes. The game also detunes each play.
- **Length and size:** keep effects under 1 s unless noted, around 20–60 KB each. The web build
  is already 16 MB gzipped.
- **License:** CC0 or your own work, to stay compatible with the GPL.
- **Repeats:** many of these can fire every second or two during auto-run. They should still be
  pleasant on the twentieth play.

## 1. In the game now

| Sound | Status | Trigger | Character | Notes |
|---|---|---|---|---|
| Landing thud | Synth | A node hits the ground | Soft, rubbery thump, like a ball on turf | 4–5 variations, 150–250 ms. Volume follows impact speed. Muted above 4× playback. |
| Anchor-foot thud | Synth | A grippy (dark) node lands | Heavier and deader, a sticky foot planting | Its own set, or a low layer under the regular thud |
| Metre bell | Synth | First time past each metre post in a run | Bright, short chime | One clean note the game repitches up a major pentatonic scale, or 5–10 pitched takes. About 1 s tail. |
| Backward bell | Synth | First time past each post behind the start | The metre bell, but glum | Repitched down a minor scale. Could be the same sample, or a duller one. |
| Slip back | Synth | Creature falls back behind the furthest post it passed | Quick descending "bwomp" | About 0.3 s. Can repeat every 0.8 s. |
| Record fanfare | Synth | A generation beats the best distance | Quick, upbeat flourish, celebratory but small | About 1 s. Can repeat every 1.5 s. |
| Regression | Synth | A generation's best is more than 5% worse than the previous one | Soft, slightly comic deflating "wah-wah" | About 1 s. Dips are normal in this algorithm, so keep it gentle. Can repeat every 1.5 s. |
| Ends behind start | Synth | A run finishes behind the start line | Sad-trombone sting | About 2 s. Plays at most every 5 s. Often heard when watching the worst creature. |
| Music loop | Synth | Always on | Gentle, curious, a little playful | 30–90 s seamless loop that sits under the effects. Ideally delivered as 2–3 stems (see Music). |

## 2. Events that happen now but make no sound yet

| Sound | Status | Trigger | Character |
|---|---|---|---|
| Hurdle hit | Planned | A node hits an obstacle (evolution3WEB preset) | Wooden knock or bonk, distinct from the ground thud |
| Muscle flex | Planned | A muscle switches between contracted and extended | Very soft creak or tick. Optional: creatures have dozens of muscles, so it needs heavy rate-limiting. |
| Generation done | Planned | Each generation finishes | Subtle tick that tolerates firing several times a second |
| Creature explodes | Planned | Physics blows up and the creature scores −∞ | Comic pop or sproing. Rare. |
| Replay start | Planned | Enter, or a new creature loads | Soft whoosh or reset blip |
| UI tick | Planned | Changing speed, best/median/worst, browsing generations | Tiny, neutral click, 2–3 variations |
| Volume step / mute | Planned | M, `,` `.` `[` `]` | Short blip at the new effects level |

## 3. Obstacle courses (screens are in; most sounds aren't)

| Sound | Status | Trigger | Character |
|---|---|---|---|
| Menu music | Planned | Level-select screen | Calmer loop, can share a theme with the in-level music |
| Menu hover / select / back | Planned | bevy_ui buttons | One consistent family of three UI sounds |
| Level start | Planned | Entering a course | Short "ready, go" sting |
| Goal reached | Synth | A generation's best reaches the level's goal | Bigger payoff than the record fanfare, 2–3 s. The placeholder is an arpeggio landing on a ringing chord. |
| Star awarded ×3 | Planned | Results screen, one per star | Rising pitch per star, so it works as a sequence |
| Results music | Planned | Results screen | 10–20 s resolving sting, or a short loop |
| Locked level | Planned | Clicking a locked level | Gentle "nope" thunk |

## 4. Later roadmap items

| Sound | Status | Feature | Character |
|---|---|---|---|
| Ghost layer | Planned | Ghost races | An airy, filtered version of the thud, or a faint shimmer while ghosts are on screen |
| Overtaking a ghost | Planned | Ghost races | Quick whoosh or passing ding |
| Parent picked / breed | Planned | Hand breeding | Selection pop, then a bubbly "splice" sound |
| Node place / muscle draw / delete | Planned | Creature editor | Tactile clicks: a pop for nodes, a stretchy twang for muscles, a crumple for delete |
| Slow motion in / out | Planned | `Time<Virtual>` slow-mo | Tape-slowdown and tape-speedup swooshes |
| Screen-shake impact | Planned | Records | Low boom layer under the fanfare |

## Music

For the in-level loop, ask for 2–3 stems (base, rhythm, melody) at the same tempo and length.
The game can then fade stems in as the best distance improves or during ghost races. The
placeholder is Am–F–C–G at 96 bpm, but the designed music doesn't need to match it.

## Where to start

The nine sounds in section 1 replace what already plays and are the quickest win. The hurdle hit
and UI tick from section 2 are cheap to add alongside them.
