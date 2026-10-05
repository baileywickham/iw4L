/// Knots an AI accuracy graph keeps (the graph files hold at most 16).
pub const AI_ACCURACY_GRAPH_MAX_KNOTS: usize = 16;

/// Distance the graphs' x axis spans: x = distance / 4000, clamped to 1.
pub const AI_ACCURACY_MAX_DISTANCE: f32 = 4000.0;

/// One weapon accuracy graph: knots of (distance fraction, accuracy), the
/// last knot at x = 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AiAccuracyGraph {
    knots: [[f32; 2]; AI_ACCURACY_GRAPH_MAX_KNOTS],
    count: u8,
}

impl AiAccuracyGraph {
    /// A graph from authored knots; fewer than two knots is no graph.
    pub fn from_knots(knots: &[[f32; 2]]) -> Option<Self> {
        let count = knots.len().min(AI_ACCURACY_GRAPH_MAX_KNOTS);
        if count < 2 {
            return None;
        }
        let mut graph = Self {
            count: count as u8,
            ..Self::default()
        };
        graph.knots[..count].copy_from_slice(&knots[..count]);
        Some(graph)
    }

    pub fn knots(&self) -> &[[f32; 2]] {
        &self.knots[..self.count as usize]
    }

    /// The graph at `fraction` (0..1): linear between the knots that bracket it.
    pub fn value_at_fraction(&self, fraction: f32) -> f32 {
        let knots = self.knots();
        let fraction = fraction.clamp(0.0, 1.0);
        for pair in knots.windows(2) {
            let ([x0, y0], [x1, y1]) = (pair[0], pair[1]);
            if x1 >= fraction {
                let t = if x1 > x0 {
                    ((fraction - x0) / (x1 - x0)).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                return y0 + (y1 - y0) * t;
            }
        }
        knots.last().map_or(0.0, |k| k[1])
    }

    /// Accuracy at `distance` units.
    pub fn value_at_distance(&self, distance: f32) -> f32 {
        self.value_at_fraction(distance / AI_ACCURACY_MAX_DISTANCE)
    }
}

/// The AI side of a weapon: its accuracy graphs (`aiVsAiAccuracyGraph`,
/// `aiVsPlayerAccuracyGraph`) and engagement ranges (`fightDist`, `maxDist`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeaponAiAccuracy {
    pub fight_dist: f32,
    pub max_dist: f32,
    pub ai_vs_ai: Option<AiAccuracyGraph>,
    pub ai_vs_player: Option<AiAccuracyGraph>,
}

impl WeaponAiAccuracy {
    pub fn is_empty(&self) -> bool {
        self.ai_vs_ai.is_none()
            && self.ai_vs_player.is_none()
            && self.fight_dist == 0.0
            && self.max_dist == 0.0
    }

    pub fn graph(&self, vs_player: bool) -> Option<&AiAccuracyGraph> {
        if vs_player {
            self.ai_vs_player.as_ref()
        } else {
            self.ai_vs_ai.as_ref()
        }
    }
}
