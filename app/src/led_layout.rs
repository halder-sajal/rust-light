//! Computes default LED screen-sampling zones from the Led page's inputs.
//!
//! Geometry (where each physical LED sits on the perimeter) only depends on
//! the per-edge counts, gaps, and sampling depth — numbering direction/start
//! position never change where a LED physically is, only which logical
//! index (1..N) it gets. So this works in two steps: lay out the canonical
//! clockwise geometry starting at the top-left corner (gap-aware), then
//! reorder that list into logical order based on `start`/`direction`.
//!
//! A gap eats into its edge's length: that edge's LEDs compact to fit in
//! (edge length - gap size), same as a real strip with a physical break.

use crate::config::{Direction, Edge, EdgeCounts, Gap, LedZone, StartPosition};

/// One entry in the canonical (always clockwise, always starting at the
/// top-left corner) walk around the perimeter.
struct CanonicalEntry {
    edge: Edge,
    index_in_edge: u32,
    zone: LedZone,
}

fn gaps_for_edge(gaps: &[Gap], edge: Edge) -> Vec<(u32, f32)> {
    let mut v: Vec<(u32, f32)> = gaps
        .iter()
        .filter(|g| g.edge == edge)
        .map(|g| (g.after_led, g.size.max(0.0)))
        .collect();
    v.sort_by_key(|(after, _)| *after);
    v
}

/// Returns (slot fraction of the edge's length, per-LED start offset as a
/// fraction of the edge's length) for `count` LEDs on one edge, given the
/// gaps on that edge (as (after_led, size) pairs sorted by after_led).
fn layout_edge(count: u32, gaps: &[(u32, f32)]) -> (f32, Vec<f32>) {
    let gap_sum: f32 = gaps.iter().map(|(_, s)| s).sum();
    let slot = 1.0 / (count as f32 + gap_sum).max(1.0);

    let mut positions = Vec::with_capacity(count as usize);
    let mut offset = 0.0f32;
    let mut gi = 0;
    for i in 0..count {
        while gi < gaps.len() && gaps[gi].0 <= i {
            offset += gaps[gi].1;
            gi += 1;
        }
        positions.push((i as f32 + offset) * slot);
    }
    (slot, positions)
}

/// Two adjacent edges (e.g. Top and Left) each normally sample a full
/// `depth`-deep strip, so near a shared corner they both claim the same
/// depth x depth square. Tapers the depth of a LED whose along-edge center
/// falls within `depth` of either end of its edge, so the two edges meet at
/// close to a diagonal seam instead of fully overlapping.
///
/// `overlap` (0.0..1.0) sets how far that taper backs off: at 0.0 a
/// corner-most LED's depth still shrinks to `MIN_FRACTION` of full depth (a
/// clean diagonal, but a thin sliver of sample area right at the corner,
/// which can miss bright content there — e.g. letterboxed video reaching
/// into the corner). At 1.0 the taper is fully disabled (every LED samples
/// the full depth again, same as before corner tapering existed). Values in
/// between blend toward more corner coverage at the cost of some overlap.
fn taper_depth(center: f32, depth: f32, overlap: f32) -> f32 {
    const MIN_FRACTION: f32 = 0.3;
    let overlap = overlap.clamp(0.0, 1.0);
    let min_fraction = MIN_FRACTION + (1.0 - MIN_FRACTION) * overlap;
    let dist_to_corner = center.min(1.0 - center);
    if dist_to_corner >= depth {
        depth
    } else {
        let min_depth = depth * min_fraction;
        min_depth + (depth - min_depth) * (dist_to_corner / depth)
    }
}

fn canonical_walk(counts: &EdgeCounts, depth: f32, overlap: f32, gaps: &[Gap]) -> Vec<CanonicalEntry> {
    let depth = depth.clamp(0.01, 0.9);
    let mut entries = Vec::with_capacity(counts.total() as usize);

    // Top: left -> right, index_in_edge 0 is leftmost. Flush with y=0;
    // height tapers near the left/right corners.
    let (slot, pos) = layout_edge(counts.top, &gaps_for_edge(gaps, Edge::Top));
    for i in 0..counts.top {
        let x = pos[i as usize];
        let h = taper_depth(x + slot / 2.0, depth, overlap);
        entries.push(CanonicalEntry {
            edge: Edge::Top,
            index_in_edge: i,
            zone: LedZone { x, y: 0.0, w: slot, h },
        });
    }

    // Right: top -> bottom, index_in_edge 0 is topmost. Flush with x=1;
    // width tapers near the top/bottom corners.
    let (slot, pos) = layout_edge(counts.right, &gaps_for_edge(gaps, Edge::Right));
    for i in 0..counts.right {
        let y = pos[i as usize];
        let w = taper_depth(y + slot / 2.0, depth, overlap);
        entries.push(CanonicalEntry {
            edge: Edge::Right,
            index_in_edge: i,
            zone: LedZone { x: 1.0 - w, y, w, h: slot },
        });
    }

    // Bottom: right -> left, index_in_edge 0 is rightmost — mirror the
    // position (not the index) so gaps land at the intended spot even once
    // slots are non-uniform. Flush with y=1; height tapers near corners.
    let (slot, pos) = layout_edge(counts.bottom, &gaps_for_edge(gaps, Edge::Bottom));
    for i in 0..counts.bottom {
        let x = 1.0 - slot - pos[i as usize];
        let h = taper_depth(x + slot / 2.0, depth, overlap);
        entries.push(CanonicalEntry {
            edge: Edge::Bottom,
            index_in_edge: i,
            zone: LedZone { x, y: 1.0 - h, w: slot, h },
        });
    }

    // Left: bottom -> top, index_in_edge 0 is bottommost — same mirroring.
    // Flush with x=0; width tapers near corners.
    let (slot, pos) = layout_edge(counts.left, &gaps_for_edge(gaps, Edge::Left));
    for i in 0..counts.left {
        let y = 1.0 - slot - pos[i as usize];
        let w = taper_depth(y + slot / 2.0, depth, overlap);
        entries.push(CanonicalEntry {
            edge: Edge::Left,
            index_in_edge: i,
            zone: LedZone { x: 0.0, y, w, h: slot },
        });
    }

    entries
}

/// Index into `canonical_walk`'s output where `start` sits.
fn canonical_index_of(entries: &[CanonicalEntry], start: StartPosition) -> usize {
    entries
        .iter()
        .position(|e| e.edge == start.edge && e.index_in_edge == start.offset)
        .unwrap_or(0)
}

/// Computes one zone per LED, in logical order (index 0 = LED #1), from the
/// current counts/direction/start/depth/gaps. `corner_overlap_pct` (0..100)
/// controls how much adjacent edges overlap near a shared corner — see
/// `taper_depth`.
pub fn compute_default_zones(
    counts: &EdgeCounts,
    direction: Direction,
    start: StartPosition,
    depth_pct: f32,
    corner_overlap_pct: f32,
    gaps: &[Gap],
) -> Vec<LedZone> {
    let entries = canonical_walk(counts, depth_pct / 100.0, corner_overlap_pct / 100.0, gaps);
    let total = entries.len();
    if total == 0 {
        return Vec::new();
    }

    let start_idx = canonical_index_of(&entries, start);
    (0..total)
        .map(|i| {
            let idx = match direction {
                Direction::Clockwise => (start_idx + i) % total,
                Direction::CounterClockwise => (start_idx + total - (i % total)) % total,
            };
            entries[idx].zone
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(top: u32, right: u32, bottom: u32, left: u32) -> EdgeCounts {
        EdgeCounts { top, right, bottom, left }
    }

    #[test]
    fn total_zones_matches_total_leds() {
        let c = counts(4, 3, 4, 3);
        let zones =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 10.0, 0.0, &[]);
        assert_eq!(zones.len(), c.total() as usize);
    }

    #[test]
    fn clockwise_from_top_left_walks_top_edge_first() {
        let c = counts(4, 2, 4, 2);
        let zones =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 10.0, 0.0, &[]);
        assert!(zones[0].x < zones[1].x);
        assert!(zones[1].x < zones[2].x);
        assert_eq!(zones[0].y, 0.0);
    }

    #[test]
    fn start_offset_mid_edge_wraps_fully_around() {
        let c = counts(10, 5, 10, 5);
        let start = StartPosition { edge: Edge::Bottom, offset: 2 };
        let zones = compute_default_zones(&c, Direction::Clockwise, start, 10.0, 0.0, &[]);
        assert_eq!(zones.len(), c.total() as usize);
        assert!((zones[0].y - 0.9).abs() < 0.001);
    }

    #[test]
    fn counter_clockwise_reverses_walk_order() {
        let c = counts(4, 2, 4, 2);
        let cw =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 10.0, 0.0, &[]);
        let ccw = compute_default_zones(
            &c,
            Direction::CounterClockwise,
            StartPosition::default(),
            10.0,
            0.0,
            &[],
        );
        assert_eq!(cw[0].x, ccw[0].x);
        assert_eq!(cw[0].y, ccw[0].y);
        assert_ne!(cw[1].x, ccw[1].x);
    }

    #[test]
    fn gap_shrinks_leds_on_its_own_edge_only() {
        let c = counts(20, 10, 20, 10);
        let gaps = [Gap { edge: Edge::Bottom, after_led: 10, size: 5.0 }];
        let with_gap =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 10.0, 0.0, &gaps);
        let without_gap =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 10.0, 0.0, &[]);

        // Top edge (LEDs 1..20 in canonical/clockwise-from-top-left order)
        // must be unaffected by a gap on Bottom.
        for i in 0..20 {
            assert_eq!(with_gap[i].w, without_gap[i].w);
        }
        // Bottom-edge LEDs must have shrunk (narrower slot than before).
        let bottom_start = 20 + 10; // after Top + Right
        assert!(with_gap[bottom_start].w < without_gap[bottom_start].w);
    }

    #[test]
    fn gap_leaves_visible_blank_space_between_neighbors() {
        let c = counts(0, 0, 20, 0);
        let gaps = [Gap { edge: Edge::Bottom, after_led: 10, size: 5.0 }];
        let zones =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 10.0, 0.0, &gaps);
        // Canonical Bottom order is right->left; after_led=10 means the gap
        // sits between canonical index_in_edge 9 and 10, i.e. between
        // zones[10] and zones[9] in this all-bottom, clockwise-from-start
        // layout (index_in_edge == logical index here since start is Top
        // with count 0, so logical order == canonical order starting at
        // Bottom's first entry).
        let left_edge_of_9 = zones[9].x;
        let right_edge_of_10 = zones[10].x + zones[10].w;
        assert!((left_edge_of_9 - right_edge_of_10).abs() > 0.01);
    }

    #[test]
    fn corner_leds_taper_depth_to_avoid_overlap() {
        // Many LEDs per edge so several land within the depth-square of the
        // top-left corner (depth=20%, slot << depth for 50 LEDs/edge).
        let c = counts(50, 50, 50, 50);
        let zones =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 20.0, 0.0, &[]);
        // zones[0] is Top's leftmost LED, i.e. the corner-most one: its
        // height must be tapered below the full depth.
        assert!(zones[0].h < 0.20, "corner LED height should taper, got h={}", zones[0].h);
        // ...but never all the way to zero — it still samples a real area.
        assert!(zones[0].h > 0.20 * 0.3 - 0.001, "taper should respect the floor, got h={}", zones[0].h);
    }

    #[test]
    fn leds_far_from_any_corner_keep_full_depth() {
        let c = counts(50, 50, 50, 50);
        let zones =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 10.0, 0.0, &[]);
        // Top edge's middle-most LED (index ~25 of 50) is far from both
        // corners and should be completely untapered.
        assert!((zones[25].h - 0.10).abs() < 0.0001, "expected full depth, got h={}", zones[25].h);
    }

    #[test]
    fn left_edge_width_tapers_near_its_corners_too() {
        let c = counts(50, 50, 50, 50);
        let zones =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 20.0, 0.0, &[]);
        // Left edge entries come after Top(50)+Right(50)+Bottom(50) = 150;
        // index_in_edge 0 is bottommost, so the last Left entry (index 199)
        // is topmost — the one closest to the top-left corner.
        let left_topmost = &zones[199];
        assert_eq!(left_topmost.x, 0.0, "left edge must stay flush with the screen's left border");
        assert!(left_topmost.w < 0.20, "corner LED width should taper, got w={}", left_topmost.w);
    }

    #[test]
    fn corner_overlap_slider_increases_corner_depth() {
        let c = counts(50, 50, 50, 50);
        let no_overlap =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 20.0, 0.0, &[]);
        let half_overlap =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 20.0, 50.0, &[]);
        let full_overlap =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 20.0, 100.0, &[]);

        // The corner-most LED's height should grow monotonically with the
        // overlap slider.
        assert!(
            no_overlap[0].h < half_overlap[0].h,
            "50% overlap should taper less than 0%: {} vs {}",
            no_overlap[0].h,
            half_overlap[0].h
        );
        assert!(
            half_overlap[0].h < full_overlap[0].h,
            "100% overlap should taper less than 50%: {} vs {}",
            half_overlap[0].h,
            full_overlap[0].h
        );
    }

    #[test]
    fn full_corner_overlap_disables_tapering_entirely() {
        let c = counts(50, 50, 50, 50);
        let zones =
            compute_default_zones(&c, Direction::Clockwise, StartPosition::default(), 20.0, 100.0, &[]);
        // At 100% overlap every LED (including the corner-most one) should
        // sample the full, untapered depth — matching pre-corner-fix
        // behavior.
        assert!((zones[0].h - 0.20).abs() < 0.0001, "expected full depth at 100% overlap, got h={}", zones[0].h);
    }
}
