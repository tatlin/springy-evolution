//! Simulation core — a line-by-line port of the physics and genetic algorithm in
//! Cary Huang's "Evolution Simulator" (Processing), as tweaked in tatlin/evolution3WEB.
//!
//! No Bevy in here: this module is deterministic for a given seed and can be run headless.
//!
//! Units follow the original: 1 unit = 20 cm, y grows *downward*, the ground surface is y = 0,
//! so a node resting on the ground has y = -radius.

use std::f32::consts::PI;

// ---------------------------------------------------------------------------------------------
// RNG (splitmix64) — tiny, dependency-free, and reproducible across platforms.
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Processing `random(lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    /// Processing `int(random(0, n))`.
    pub fn index(&mut self, n: usize) -> usize {
        ((self.unit() * n as f32) as usize).min(n.saturating_sub(1))
    }
    /// Original `r()`: `pow(random(-1,1), 19)` — almost always tiny, occasionally large.
    pub fn r(&mut self) -> f32 {
        self.range(-1.0, 1.0).powi(19)
    }
    /// Original `rInt()`: `int(random(-0.01, 1.01))` — 0 most of the time, 1 about 1% of the time.
    pub fn r_int(&mut self) -> i32 {
        self.range(-0.01, 1.01) as i32
    }
}

// ---------------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl Rect {
    /// Corners in any order. (The hurdles in evolution3WEB are written with y1 > y2, which makes
    /// the original collision test silently fail — they are drawn but never touched. We normalize.)
    pub fn new(ax: f32, ay: f32, bx: f32, by: f32) -> Self {
        Self { x1: ax.min(bx), y1: ay.min(by), x2: ax.max(bx), y2: ay.max(by) }
    }
}

/// How the first generation is built.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeedMode {
    /// Cary Huang's original: 4–7 random nodes, random muscles.
    Random,
    /// tatlin's "MDJ_Truss": a rows×cols grid of nodes braced with diagonals.
    Truss { rows: usize, cols: usize },
}

/// What happens to a parent's muscles when it has a mutated child.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MuscleInheritance {
    /// Cary Huang's original: every muscle is copied and mutated, and nodes/muscles can be
    /// added or removed.
    Mutate,
    /// evolution3WEB: the muscle-mutation loop is commented out, so the child starts with *no*
    /// muscles and `checkForLoneNodes()` regrows one random muscle per node. Only node positions
    /// and frictions are actually inherited.
    Regrow,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub min_node_size: f32,
    pub max_node_size: f32,
    pub min_node_friction: f32,
    pub max_node_friction: f32,
    pub gravity: f32,
    pub air_friction: f32,
    pub mutability_factor: f32,
    pub friction: f32,
    pub have_ground: bool,
    pub rects: Vec<Rect>,
    pub population: usize,
    /// 900 steps = 15 s at 60 fps.
    pub sim_steps: u32,
    pub seed_mode: SeedMode,
    pub inheritance: MuscleInheritance,
    /// Longest rest length a newly grown muscle may have. Without it, regrown muscles copy
    /// whatever distance their nodes have drifted to, and creatures sprawl without bound.
    pub max_muscle_length: f32,
    /// Whether children can gain or lose nodes and muscles. The original only does this in
    /// Cary Huang's version; here it is on for both presets.
    pub mutate_topology: bool,
    /// After mutation, give every node muscles to at least this many others (farthest first, so
    /// opposite corners get braced). 0 keeps the original's sparse regrowth.
    pub min_connections: usize,
}

impl Config {
    /// The settings in tatlin/evolution3WEB.
    pub fn evolution3web() -> Self {
        Self {
            min_node_size: 0.4,
            max_node_size: 0.4,
            min_node_friction: 0.0,
            max_node_friction: 1.0,
            gravity: 0.005,
            air_friction: 0.95,
            mutability_factor: 1.0,
            friction: 4.0,
            have_ground: true,
            rects: vec![Rect::new(2.0, -0.1, 3.0, -1.0), Rect::new(4.0, -0.1, 5.0, -1.0)],
            population: 1000,
            sim_steps: 900,
            seed_mode: SeedMode::Truss { rows: 2, cols: 2 },
            inheritance: MuscleInheritance::Regrow,
            max_muscle_length: 2.0,
            mutate_topology: true,
            min_connections: 3,
        }
    }

    /// Cary Huang's original sketch: random creatures, full mutation, flat ground.
    pub fn cary_huang() -> Self {
        Self {
            rects: vec![],
            seed_mode: SeedMode::Random,
            inheritance: MuscleInheritance::Mutate,
            min_connections: 0,
            ..Self::evolution3web()
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Genome
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Diameter (the original calls it mass, and it is used as both).
    pub m: f32,
    /// Ground friction, 0..1.
    pub f: f32,
}

impl Node {
    fn new(x: f32, y: f32, m: f32, f: f32) -> Self {
        Self { x, y, vx: 0.0, vy: 0.0, m, f }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Muscle {
    pub period: i32,
    pub c1: usize,
    pub c2: usize,
    pub contract_time: f32,
    pub extend_time: f32,
    pub contract_length: f32,
    pub extend_length: f32,
    pub contracted: bool,
    pub rigidity: f32,
}

#[derive(Clone, Debug)]
pub struct Creature {
    pub id: u64,
    pub nodes: Vec<Node>,
    pub muscles: Vec<Muscle>,
    /// Distance travelled in metres after `sim_steps` (0 until evaluated).
    pub fitness: f32,
    pub alive: bool,
    /// The original `creatureTimer`: frames per muscle cycle ("heartbeat").
    pub heartbeat: f32,
    pub mutability: f32,
}

impl Creature {
    /// Original species code: tens digit = nodes % 10, ones digit = muscles % 10.
    pub fn species(&self) -> usize {
        (self.nodes.len() % 10) * 10 + self.muscles.len() % 10
    }

    pub fn random(rng: &mut Rng, cfg: &Config, id: u64) -> Self {
        let node_num = rng.range(4.0, 8.0) as usize;
        let muscle_num = rng.range((node_num - 1) as f32, (node_num * 4 - 8) as f32) as usize;
        let mut nodes = Vec::with_capacity(node_num);
        for _ in 0..node_num {
            nodes.push(Node::new(
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(cfg.min_node_size, cfg.max_node_size),
                rng.range(cfg.min_node_friction, cfg.max_node_friction),
            ));
        }
        let mut muscles = Vec::with_capacity(muscle_num);
        for i in 0..muscle_num {
            let (c1, c2) = if i < node_num - 1 {
                (i, i + 1) // a chain first, so everything is connected
            } else {
                let c1 = rng.index(node_num);
                let mut c2 = c1;
                while c2 == c1 {
                    c2 = rng.index(node_num);
                }
                (c1, c2)
            };
            let l1 = rng.range(0.5, 1.5);
            let l2 = rng.range(0.5, 1.5);
            let t1 = rng.unit();
            let t2 = rng.unit();
            muscles.push(Muscle {
                period: rng.range(1.0, 3.0) as i32,
                c1,
                c2,
                contract_time: t1,
                extend_time: t2,
                contract_length: l1.min(l2),
                extend_length: l1.max(l2),
                contracted: t1 <= t2,
                rigidity: rng.range(0.02, 0.08),
            });
        }
        let mut c = Creature {
            id,
            nodes,
            muscles,
            fitness: 0.0,
            alive: true,
            heartbeat: rng.range(40.0, 80.0),
            mutability: 1.0,
        };
        c.settle(cfg);
        c.check_for_overlap(true);
        c.check_for_lone_nodes(rng, cfg);
        c
    }

    /// tatlin's "MDJ_Truss" seed creature.
    pub fn truss(rng: &mut Rng, cfg: &Config, id: u64, rows: usize, cols: usize) -> Self {
        let mut nodes = Vec::new();
        for i in 0..rows {
            for j in 0..cols {
                nodes.push(Node::new(
                    j as f32,
                    -(i as f32),
                    rng.range(cfg.min_node_size, cfg.max_node_size),
                    rng.range(cfg.min_node_friction, cfg.max_node_friction),
                ));
            }
        }
        // Muscle(2, pt1, pt2, contractTime 1, extendTime 1, contractLength 1, extendLength 2,
        // contracted, rigidity 0.1). With equal times the muscle is permanently contracted: a bar.
        let bar = |c1, c2| Muscle {
            period: 2,
            c1,
            c2,
            contract_time: 1.0,
            extend_time: 1.0,
            contract_length: 1.0,
            extend_length: 2.0,
            contracted: true,
            rigidity: 0.1,
        };
        let mut muscles = Vec::new();
        for i in 0..rows {
            for j in 0..cols.saturating_sub(1) {
                muscles.push(bar(i * cols + j, i * cols + j + 1)); // horizontal
            }
            if i > 0 {
                for j in 0..cols {
                    muscles.push(bar(i * cols + j, j)); // vertical (to row 0, as in the original)
                }
                for j in 0..cols.saturating_sub(1) {
                    muscles.push(bar(i * cols + j + 1, (i - 1) * cols + j)); // diagonals
                    muscles.push(bar(i * cols + j, (i - 1) * cols + j + 1));
                }
            }
        }
        let mut c = Creature {
            id,
            nodes,
            muscles,
            fitness: 0.0,
            alive: true,
            heartbeat: rng.range(40.0, 80.0),
            mutability: 1.0,
        };
        c.settle(cfg);
        c
    }

    /// `toStableConfiguration()` + `adjustToCenter()`.
    pub fn settle(&mut self, cfg: &Config) {
        for _ in 0..200 {
            for mu in &self.muscles {
                let target = if mu.contracted { mu.contract_length } else { mu.extend_length };
                apply_muscle(&mut self.nodes, mu, target);
            }
            for n in &mut self.nodes {
                n.vx *= cfg.air_friction;
                n.vy *= cfg.air_friction;
                n.x += n.vx;
                n.y += n.vy;
            }
        }
        for n in &mut self.nodes {
            n.vx = 0.0;
            n.vy = 0.0;
        }
        if self.nodes.is_empty() {
            return;
        }
        let avx = self.nodes.iter().map(|n| n.x).sum::<f32>() / self.nodes.len() as f32;
        let low_y = self.nodes.iter().map(|n| n.y + n.m / 2.0).fold(-1000.0, f32::max);
        for n in &mut self.nodes {
            n.x -= avx;
            n.y -= low_y;
        }
    }

    /// The original `modified()` — makes a mutated child.
    pub fn mutated(&self, rng: &mut Rng, cfg: &Config, id: u64) -> Creature {
        let mf = cfg.mutability_factor;
        let mu = self.mutability;
        let mut child = Creature {
            id,
            nodes: Vec::with_capacity(self.nodes.len() + 1),
            muscles: Vec::with_capacity(self.muscles.len() + 1),
            fitness: 0.0,
            alive: true,
            heartbeat: self.heartbeat + rng.r() * 16.0 * mu * mf,
            mutability: (mu * mf * rng.range(0.8, 1.25)).min(2.0),
        };
        for n in &self.nodes {
            let nm = (n.m + rng.r() * 0.1 * mu * mf).clamp(cfg.min_node_size, cfg.max_node_size);
            let nf = (n.f + rng.r() * 0.1 * mu * mf)
                .clamp(cfg.min_node_friction, cfg.max_node_friction);
            child.nodes.push(Node::new(
                n.x + rng.r() * 0.5 * mu * mf,
                n.y + rng.r() * 0.5 * mu * mf,
                nm,
                nf,
            ));
        }
        if cfg.inheritance == MuscleInheritance::Mutate {
            let node_num = self.nodes.len();
            for m in &self.muscles {
                child.muscles.push(mutate_muscle(m, node_num, mu * mf, rng));
            }
        }
        if cfg.mutate_topology {
            let p = 0.04 * mu * mf;
            if rng.unit() < p || child.nodes.len() <= 2 {
                child.add_random_node(rng, cfg);
            }
            if rng.unit() < p {
                child.add_random_muscle(rng, cfg, None);
            }
            if rng.unit() < p && child.nodes.len() >= 4 {
                child.remove_random_node(rng);
            }
            if rng.unit() < p && child.muscles.len() >= 2 {
                child.remove_random_muscle(rng);
            }
            child.check_for_overlap(true);
        }
        child.check_for_lone_nodes(rng, cfg);
        child.brace(rng, cfg);
        child.settle(cfg);
        child
    }

    fn check_for_overlap(&mut self, remove: bool) {
        let mut bad = vec![false; self.muscles.len()];
        for i in 0..self.muscles.len() {
            let a = self.muscles[i];
            if a.c1 == a.c2 {
                bad[i] = true;
            }
            for b in &self.muscles[i + 1..] {
                if (a.c1 == b.c1 && a.c2 == b.c2) || (a.c1 == b.c2 && a.c2 == b.c1) {
                    bad[i] = true;
                }
            }
        }
        if remove {
            let mut i = 0;
            self.muscles.retain(|_| {
                i += 1;
                !bad[i - 1]
            });
        }
    }

    fn check_for_lone_nodes(&mut self, rng: &mut Rng, cfg: &Config) {
        let n = self.nodes.len();
        if n < 3 {
            return;
        }
        for i in 0..n {
            let mut connections = 0;
            let mut connected_to = None;
            for m in &self.muscles {
                if m.c1 == i {
                    connections += 1;
                    connected_to = Some(m.c2);
                } else if m.c2 == i {
                    connections += 1;
                    connected_to = Some(m.c1);
                }
            }
            if connections <= 1 {
                let mut other = rng.index(n);
                while other == i || Some(other) == connected_to {
                    other = rng.index(n);
                }
                self.add_random_muscle(rng, cfg, Some((i, other)));
            }
        }
    }

    /// Connect each node to at least `cfg.min_connections` others, always to the farthest node
    /// it isn't already joined to, so cross-bracing spans opposite corners.
    fn brace(&mut self, rng: &mut Rng, cfg: &Config) {
        let n = self.nodes.len();
        let want = cfg.min_connections.min(n.saturating_sub(1));
        for i in 0..n {
            loop {
                let partners: Vec<usize> = self
                    .muscles
                    .iter()
                    .filter_map(|m| match (m.c1 == i, m.c2 == i) {
                        (true, false) => Some(m.c2),
                        (false, true) => Some(m.c1),
                        _ => None,
                    })
                    .collect();
                if partners.len() >= want {
                    break;
                }
                let a = self.nodes[i];
                let dist = |j: &usize| (self.nodes[*j].x - a.x).hypot(self.nodes[*j].y - a.y);
                let farthest = (0..n)
                    .filter(|j| *j != i && !partners.contains(j))
                    .max_by(|j, k| dist(j).total_cmp(&dist(k)));
                let Some(j) = farthest else { break };
                self.add_random_muscle(rng, cfg, Some((i, j)));
            }
        }
    }

    fn add_random_node(&mut self, rng: &mut Rng, cfg: &Config) {
        let parent = rng.index(self.nodes.len());
        let ang = rng.range(0.0, 2.0 * PI);
        let d = rng.unit().sqrt();
        let x = self.nodes[parent].x + ang.cos() * 0.5 * d;
        let y = self.nodes[parent].y + ang.sin() * 0.5 * d;
        self.nodes.push(Node::new(
            x,
            y,
            rng.range(cfg.min_node_size, cfg.max_node_size),
            rng.range(cfg.min_node_friction, cfg.max_node_friction),
        ));
        let new = self.nodes.len() - 1;
        let mut closest = 0;
        let mut record = f32::MAX;
        for (i, n) in self.nodes[..new].iter().enumerate() {
            if i != parent {
                let d = ((n.x - x).powi(2) + (n.y - y).powi(2)).sqrt();
                if d < record {
                    record = d;
                    closest = i;
                }
            }
        }
        self.add_random_muscle(rng, cfg, Some((parent, new)));
        self.add_random_muscle(rng, cfg, Some((closest, new)));
    }

    fn add_random_muscle(&mut self, rng: &mut Rng, cfg: &Config, ends: Option<(usize, usize)>) {
        let (c1, c2) = ends.unwrap_or_else(|| {
            let c1 = rng.index(self.nodes.len());
            let mut c2 = c1;
            while c2 == c1 && self.nodes.len() >= 2 {
                c2 = rng.index(self.nodes.len());
            }
            (c1, c2)
        });
        let (a, b) = (self.nodes[c1], self.nodes[c2]);
        let dist = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
        let ratio = rng.range(0.01, 0.2);
        // Cap the base length so even the extended muscle stays within `max_muscle_length`;
        // `settle()` then pulls far-apart nodes back together.
        let dist = dist.min(cfg.max_muscle_length / (1.0 + ratio));
        let t1 = rng.unit();
        let t2 = rng.unit();
        self.muscles.push(Muscle {
            period: rng.range(1.0, 3.0) as i32,
            c1,
            c2,
            contract_time: t1,
            extend_time: t2,
            contract_length: dist * (1.0 - ratio),
            extend_length: dist * (1.0 + ratio),
            contracted: t1 <= t2,
            rigidity: rng.range(0.02, 0.08),
        });
    }

    fn remove_random_node(&mut self, rng: &mut Rng) {
        let choice = rng.index(self.nodes.len());
        self.nodes.remove(choice);
        self.muscles.retain(|m| m.c1 != choice && m.c2 != choice);
        for m in &mut self.muscles {
            if m.c1 >= choice {
                m.c1 -= 1;
            }
            if m.c2 >= choice {
                m.c2 -= 1;
            }
        }
    }

    fn remove_random_muscle(&mut self, rng: &mut Rng) {
        let choice = rng.index(self.muscles.len());
        self.muscles.remove(choice);
    }
}

/// The original `modifyMuscle()`.
fn mutate_muscle(m: &Muscle, node_num: usize, mu: f32, rng: &mut Rng) -> Muscle {
    let mut c1 = m.c1;
    let mut c2 = m.c2;
    if rng.unit() < 0.02 * mu {
        c1 = rng.index(node_num);
    }
    if rng.unit() < 0.02 * mu {
        c2 = rng.index(node_num);
    }
    let rigidity = (m.rigidity * (1.0 + rng.r() * 0.9 * mu)).clamp(0.01, 0.08);
    let max_change = 1.0 + 0.025 / rigidity;
    let cl = (m.contract_length + rng.r() * mu).clamp(0.4, 2.0);
    let el = (m.extend_length + rng.r() * mu).clamp(0.4, 2.0);
    let cl2 = cl.min(el);
    let el2 = cl.max(el).min(cl2 * max_change);
    let mut ct = m.contract_time;
    let mut et = m.extend_time;
    if rng.unit() < 0.5 {
        ct = ((m.contract_time - m.extend_time) * rng.r() * mu + ct + 1.0) % 1.0;
    } else {
        et = ((m.extend_time - m.contract_time) * rng.r() * mu + et + 1.0) % 1.0;
    }
    Muscle {
        period: (m.period + rng.r_int()).max(0),
        c1,
        c2,
        contract_time: ct,
        extend_time: et,
        contract_length: cl2,
        extend_length: el2,
        contracted: ct <= et,
        rigidity,
    }
}

// ---------------------------------------------------------------------------------------------
// Physics
// ---------------------------------------------------------------------------------------------

/// `Muscle.applyForce()`: a clamped linear spring that only touches velocities.
fn apply_muscle(nodes: &mut [Node], mu: &Muscle, target: f32) {
    if mu.c1 >= nodes.len() || mu.c2 >= nodes.len() {
        return;
    }
    let (a, b) = (nodes[mu.c1], nodes[mu.c2]);
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    let dist = (dx * dx + dy * dy).sqrt();
    let (s, c) = dy.atan2(dx).sin_cos();
    let force = (1.0 - dist / target).clamp(-0.4, 0.4);
    nodes[mu.c1].vx += c * force * mu.rigidity / a.m;
    nodes[mu.c1].vy += s * force * mu.rigidity / a.m;
    nodes[mu.c2].vx -= c * force * mu.rigidity / b.m;
    nodes[mu.c2].vy -= s * force * mu.rigidity / b.m;
}

/// `Node.hitWalls()`: ground contact with its odd friction model, plus rectangle obstacles.
fn hit_walls(n: &mut Node, cfg: &Config) {
    let friction = cfg.friction;
    let dif = n.y + n.m / 2.0;
    if dif >= 0.0 && cfg.have_ground {
        n.y = -n.m / 2.0;
        n.vy = 0.0;
        n.x -= n.vx * n.f;
        if n.vx > 0.0 {
            n.vx = (n.vx - n.f * dif * friction).max(0.0);
        } else {
            n.vx = (n.vx + n.f * dif * friction).min(0.0);
        }
    }
    for r in &cfg.rects {
        if (n.x - (r.x1 + r.x2) / 2.0).abs() > (r.x2 - r.x1 + n.m) / 2.0
            || (n.y - (r.y1 + r.y2) / 2.0).abs() > (r.y2 - r.y1 + n.m) / 2.0
        {
            continue;
        }
        let mut flip = false;
        let (px, py, section);
        if n.x >= r.x1 && n.x < r.x2 && n.y >= r.y1 && n.y < r.y2 {
            // Centre is inside: push out through the nearest face.
            let d1 = n.x - r.x1;
            let d2 = r.x2 - n.x;
            let d3 = n.y - r.y1;
            let d4 = r.y2 - n.y;
            if d1 < d2 && d1 < d3 && d1 < d4 {
                (px, py, section) = (r.x1, n.y, 3);
            } else if d2 < d3 && d2 < d4 {
                (px, py, section) = (r.x2, n.y, 5);
            } else if d3 < d4 {
                (px, py, section) = (n.x, r.y1, 1);
            } else {
                (px, py, section) = (n.x, r.y2, 7);
            }
            flip = true;
        } else {
            let (sx, cx) = if n.x < r.x1 {
                (0, r.x1)
            } else if n.x < r.x2 {
                (1, n.x)
            } else {
                (2, r.x2)
            };
            let (sy, cy) = if n.y < r.y1 {
                (0, r.y1)
            } else if n.y < r.y2 {
                (3, n.y)
            } else {
                (6, r.y2)
            };
            (px, py, section) = (cx, cy, sx + sy);
        }
        let distance = ((n.x - px).powi(2) + (n.y - py).powi(2)).sqrt();
        let rad = n.m / 2.0;
        let mut wall_angle;
        if distance <= 1e-8 {
            wall_angle = match section {
                0..=2 => PI / 4.0 + section as f32 * PI / 4.0,
                6.. => 5.0 * PI / 4.0 + (8 - section) as f32 * PI / 4.0,
                3 => PI,
                _ => 0.0,
            };
            flip = false;
        } else {
            wall_angle = (py - n.y).atan2(px - n.x);
        }
        if flip {
            wall_angle += PI;
        }
        if distance < rad || flip {
            let dif = rad - distance;
            if distance <= 1e-8 {
                // The original divides by zero here and the creature turns into NaN.
                n.x = px - wall_angle.cos() * rad;
                n.y = py - wall_angle.sin() * rad;
            } else {
                let multi = if flip { -rad / distance } else { rad / distance };
                n.x = (n.x - px) * multi + px;
                n.y = (n.y - py) * multi + py;
            }
            let velo_angle = n.vy.atan2(n.vx);
            let velo_mag = (n.vx * n.vx + n.vy * n.vy).sqrt();
            let rel = velo_angle - wall_angle;
            let rel_y = rel.sin() * velo_mag * dif * friction;
            n.vx = -rel.sin() * rel_y;
            n.vy = rel.cos() * rel_y;
        }
    }
}

/// A running copy of a creature — the original's global `n`/`m` lists + `simulate()`.
#[derive(Clone)]
pub struct Sim {
    pub nodes: Vec<Node>,
    pub muscles: Vec<Muscle>,
    pub timer: u32,
    heartbeat: f32,
}

impl Sim {
    pub fn new(c: &Creature) -> Self {
        Self {
            nodes: c.nodes.iter().map(|n| Node { vx: 0.0, vy: 0.0, ..*n }).collect(),
            muscles: c.muscles.clone(),
            timer: 0,
            heartbeat: c.heartbeat,
        }
    }

    pub fn step(&mut self, cfg: &Config) {
        let t = self.timer as f32;
        for i in 0..self.muscles.len() {
            let mu = &mut self.muscles[i];
            let thru = ((t / self.heartbeat) / mu.period as f32) % 1.0;
            let (ct, et) = (mu.contract_time, mu.extend_time);
            mu.contracted = (thru <= et && et <= ct)
                || (ct <= thru && thru <= et)
                || (et <= ct && ct <= thru);
            let target = if mu.contracted { mu.contract_length } else { mu.extend_length };
            let mu = *mu;
            apply_muscle(&mut self.nodes, &mu, target);
        }
        for n in &mut self.nodes {
            n.vx *= cfg.air_friction;
            n.vy *= cfg.air_friction;
            n.y += n.vy;
            n.x += n.vx;
            n.vy += cfg.gravity;
            hit_walls(n, cfg);
        }
        self.timer += 1;
    }

    pub fn center_x(&self) -> f32 {
        self.nodes.iter().map(|n| n.x).sum::<f32>() / self.nodes.len().max(1) as f32
    }

    /// Distance in metres (5 units = 1 m).
    pub fn distance_m(&self) -> f32 {
        self.center_x() * 0.2
    }
}

pub fn evaluate(c: &Creature, cfg: &Config) -> f32 {
    let mut s = Sim::new(c);
    for _ in 0..cfg.sim_steps {
        s.step(cfg);
    }
    let d = s.distance_m();
    if d.is_finite() { d } else { f32::NEG_INFINITY }
}

// ---------------------------------------------------------------------------------------------
// Genetic algorithm
// ---------------------------------------------------------------------------------------------

/// Ranks recorded each generation, as in the original percentile graph
/// (index 0 = best / 100th percentile, 14 = median, 28 = worst).
pub const PERCENTILE_RANKS: [usize; 29] = [
    0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 200, 300, 400, 500, 600, 700, 800, 900, 910, 920,
    930, 940, 950, 960, 970, 980, 990, 999,
];

#[derive(Clone)]
pub struct GenStats {
    pub generation: u32,
    pub percentiles: [f32; 29],
    pub best: Creature,
    pub median: Creature,
    pub worst: Creature,
    pub top_species: usize,
}

pub struct Population {
    pub cfg: Config,
    pub creatures: Vec<Creature>,
    /// Number of generations evaluated so far.
    pub generation: u32,
    pub history: Vec<GenStats>,
    rng: Rng,
    next_id: u64,
}

impl Population {
    pub fn new(cfg: Config, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let creatures = (0..cfg.population as u64)
            .map(|id| match cfg.seed_mode {
                SeedMode::Random => Creature::random(&mut rng, &cfg, id),
                SeedMode::Truss { rows, cols } => Creature::truss(&mut rng, &cfg, id, rows, cols),
            })
            .collect();
        Self {
            next_id: cfg.population as u64,
            cfg,
            creatures,
            generation: 0,
            history: Vec::new(),
            rng,
        }
    }

    /// Evaluate on all cores. Each creature's run is independent, so results are deterministic.
    fn evaluate_all(&mut self) {
        let cfg = &self.cfg;
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let chunk = self.creatures.len().div_ceil(threads).max(1);
        std::thread::scope(|s| {
            for part in self.creatures.chunks_mut(chunk) {
                s.spawn(move || {
                    for c in part {
                        c.fitness = evaluate(c, cfg);
                    }
                });
            }
        });
    }

    /// One full generation: test → sort → kill → reproduce.
    pub fn step(&mut self) {
        self.evaluate_all();
        // Best first.
        self.creatures.sort_by(|a, b| b.fitness.total_cmp(&a.fitness));
        let pop = self.creatures.len();
        let rank = |r: usize| (r * pop / 1000).min(pop - 1);

        let mut percentiles = [0.0; 29];
        for (k, &r) in PERCENTILE_RANKS.iter().enumerate() {
            percentiles[k] = self.creatures[rank(r)].fitness;
        }
        let mut species = [0usize; 100];
        for c in &self.creatures {
            species[c.species()] += 1;
        }
        let top_species = (0..100).max_by_key(|&i| (species[i], usize::MAX - i)).unwrap_or(0);
        self.history.push(GenStats {
            generation: self.generation,
            percentiles,
            best: self.creatures[0].clone(),
            median: self.creatures[pop / 2 - 1].clone(),
            worst: self.creatures[pop - 1].clone(),
            top_species,
        });

        // Kill: pair rank j with rank (pop-1-j); the faster one usually survives,
        // with an upset probability that grows toward the middle of the pack.
        let half = pop / 2;
        for j in 0..half {
            let f = j as f32 / pop as f32;
            let u = self.rng.range(-1.0, 1.0);
            let fast_survives = f <= (u.powi(3) + 1.0) / 2.0;
            let (live, die) = if fast_survives { (j, pop - 1 - j) } else { (pop - 1 - j, j) };
            self.creatures[live].alive = true;
            self.creatures[die].alive = false;
        }

        // Reproduce: each survivor is copied once and has one mutated child in the dead slot.
        for j in 0..half {
            let parent = if self.creatures[j].alive { j } else { pop - 1 - j };
            let dead = pop - 1 - parent;
            let child_id = self.next_id;
            let copy_id = self.next_id + 1;
            self.next_id += 2;
            let child = self.creatures[parent].mutated(&mut self.rng, &self.cfg, child_id);
            self.creatures[dead] = child;
            let p = &mut self.creatures[parent];
            p.id = copy_id;
            p.fitness = 0.0;
        }
        self.generation += 1;
    }

    pub fn latest(&self) -> Option<&GenStats> {
        self.history.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evolves_forward() {
        for cfg in [Config::cary_huang(), Config::evolution3web()] {
            let mut cfg = cfg;
            cfg.population = 200;
            let mut pop = Population::new(cfg, 7);
            for _ in 0..15 {
                pop.step();
            }
            let first = pop.history.first().unwrap().percentiles[14];
            let last = pop.history.last().unwrap().percentiles[14];
            assert!(last.is_finite());
            assert!(last >= first, "median went from {first} to {last}");
        }
    }

    #[test]
    fn topology_mutates_in_both_presets() {
        for cfg in [Config::cary_huang(), Config::evolution3web()] {
            let mut cfg = cfg;
            cfg.population = 200;
            let mut pop = Population::new(cfg, 42);
            for _ in 0..30 {
                pop.step();
            }
            let first = pop.creatures[0].nodes.len();
            assert!(pop.creatures.iter().any(|c| c.nodes.len() != first), "node count never changed");
        }
    }

    #[test]
    fn creatures_stay_compact() {
        for cfg in [Config::cary_huang(), Config::evolution3web()] {
            let mut cfg = cfg;
            cfg.population = 200;
            let mut pop = Population::new(cfg, 42);
            for _ in 0..150 {
                pop.step();
            }
            for c in &pop.creatures {
                let (lo, hi) = c.nodes.iter().fold((f32::MAX, f32::MIN), |(lo, hi), n| {
                    (lo.min(n.x.min(n.y)), hi.max(n.x.max(n.y)))
                });
                assert!(hi - lo < 10.0, "creature {} spans {} units", c.id, hi - lo);
            }
        }
    }
}
