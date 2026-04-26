use super::consts::{
    T1_SIGMA_E, T1_SIGMA_N, T1_SIGMA_NE, T1_SIGMA_NW, T1_SIGMA_S, T1_SIGMA_SE, T1_SIGMA_SW,
    T1_SIGMA_W,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlagGrid {
    width: usize,
    height: usize,
    significant: Vec<bool>,
    sign_bits: Vec<u8>,
    refinement_history: Vec<bool>,
    /// Per-bitplane "visited" (PI) flag: set by the SP pass, cleared by Cleanup.
    visited: Vec<bool>,
}

impl FlagGrid {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            significant: vec![false; len],
            sign_bits: vec![0; len],
            refinement_history: vec![false; len],
            visited: vec![false; len],
        }
    }

    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    #[inline]
    fn sample(&self, x: isize, y: isize) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as isize || y >= self.height as isize {
            None
        } else {
            Some(self.index(x as usize, y as usize))
        }
    }

    pub(crate) fn is_significant(&self, x: usize, y: usize) -> bool {
        self.significant[self.index(x, y)]
    }

    pub(crate) fn has_refinement_history(&self, x: usize, y: usize) -> bool {
        self.refinement_history[self.index(x, y)]
    }

    pub(crate) fn is_visited(&self, x: usize, y: usize) -> bool {
        self.visited[self.index(x, y)]
    }

    pub(crate) fn mark_significant(&mut self, x: usize, y: usize, sign_bit: u8) {
        let index = self.index(x, y);
        self.significant[index] = true;
        self.sign_bits[index] = sign_bit;
    }

    pub(crate) fn mark_refined(&mut self, x: usize, y: usize) {
        let index = self.index(x, y);
        self.refinement_history[index] = true;
    }

    pub(crate) fn mark_visited(&mut self, x: usize, y: usize) {
        let index = self.index(x, y);
        self.visited[index] = true;
    }

    pub(crate) fn clear_visited(&mut self, x: usize, y: usize) {
        let index = self.index(x, y);
        self.visited[index] = false;
    }

    pub(crate) fn neighbour_mask(&self, x: usize, y: usize) -> u32 {
        // Fast path: interior points (no bounds checking needed)
        if x > 0 && y > 0 && x < self.width - 1 && y < self.height - 1 {
            unsafe { self.neighbour_mask_unchecked(x, y) }
        } else {
            // Slow path: edge points with bounds checking
            self.neighbour_mask_safe(x, y)
        }
    }

    /// Fast unchecked version for interior points.
    /// SAFETY: Caller must ensure 0 < x < width-1 and 0 < y < height-1
    #[inline(always)]
    unsafe fn neighbour_mask_unchecked(&self, x: usize, y: usize) -> u32 {
        let w = self.width;
        let base = y * w + x;
        let sig = self.significant.as_ptr();

        let mut mask = 0u32;

        // NW: (x-1, y-1)
        if *sig.add(base - w - 1) { mask |= T1_SIGMA_NW; }
        // N: (x, y-1)
        if *sig.add(base - w) { mask |= T1_SIGMA_N; }
        // NE: (x+1, y-1)
        if *sig.add(base - w + 1) { mask |= T1_SIGMA_NE; }
        // W: (x-1, y)
        if *sig.add(base - 1) { mask |= T1_SIGMA_W; }
        // E: (x+1, y)
        if *sig.add(base + 1) { mask |= T1_SIGMA_E; }
        // SW: (x-1, y+1)
        if *sig.add(base + w - 1) { mask |= T1_SIGMA_SW; }
        // S: (x, y+1)
        if *sig.add(base + w) { mask |= T1_SIGMA_S; }
        // SE: (x+1, y+1)
        if *sig.add(base + w + 1) { mask |= T1_SIGMA_SE; }

        mask
    }

    /// Safe version with bounds checking for edge points.
    #[inline]
    fn neighbour_mask_safe(&self, x: usize, y: usize) -> u32 {
        let x = x as isize;
        let y = y as isize;
        let mut mask = 0u32;
        let neighbours = [
            (-1, -1, T1_SIGMA_NW),
            (0, -1, T1_SIGMA_N),
            (1, -1, T1_SIGMA_NE),
            (-1, 0, T1_SIGMA_W),
            (1, 0, T1_SIGMA_E),
            (-1, 1, T1_SIGMA_SW),
            (0, 1, T1_SIGMA_S),
            (1, 1, T1_SIGMA_SE),
        ];
        for (dx, dy, bit) in neighbours {
            if let Some(index) = self.sample(x + dx, y + dy) {
                if self.significant[index] {
                    mask |= bit;
                }
            }
        }
        mask
    }

    /// Returns `true` when the AGG path applies for the cleanup pass: every
    /// sample in the column-stripe `x` at rows `k..k+lim` has no significant
    /// neighbours, is itself not significant, and has not been visited by the
    /// current bitplane's SP pass.  Equivalent to `*f == 0` in OpenJPEG.
    pub(crate) fn stripe_is_clean(&self, x: usize, k: usize, lim: usize) -> bool {
        for ci in 0..lim {
            let y = k + ci;
            if y >= self.height {
                break;
            }
            if self.is_significant(x, y) || self.is_visited(x, y) || self.neighbour_mask(x, y) != 0
            {
                return false;
            }
        }
        true
    }

    pub(crate) fn cardinal_sign_context(&self, x: usize, y: usize) -> (u32, u8) {
        // Fast path: interior points
        if x > 0 && y > 0 && x < self.width - 1 && y < self.height - 1 {
            unsafe { self.cardinal_sign_context_unchecked(x, y) }
        } else {
            self.cardinal_sign_context_safe(x, y)
        }
    }

    /// Fast unchecked version for interior points.
    /// SAFETY: Caller must ensure 0 < x < width-1 and 0 < y < height-1
    #[inline(always)]
    unsafe fn cardinal_sign_context_unchecked(&self, x: usize, y: usize) -> (u32, u8) {
        let w = self.width;
        let base = y * w + x;
        let sig = self.significant.as_ptr();
        let signs = self.sign_bits.as_ptr();

        let west_sig = *sig.add(base - 1);
        let west_sign = *signs.add(base - 1);
        let north_sig = *sig.add(base - w);
        let north_sign = *signs.add(base - w);
        let east_sig = *sig.add(base + 1);
        let east_sign = *signs.add(base + 1);
        let south_sig = *sig.add(base + w);
        let south_sign = *signs.add(base + w);

        (
            super::helpers::sign_lut_index(
                west_sig, west_sign,
                north_sig, north_sign,
                east_sig, east_sign,
                south_sig, south_sign,
            ),
            *signs.add(base),
        )
    }

    #[inline]
    fn cardinal_sign_context_safe(&self, x: usize, y: usize) -> (u32, u8) {
        let x = x as isize;
        let y = y as isize;
        let west = self.sample(x - 1, y);
        let north = self.sample(x, y - 1);
        let east = self.sample(x + 1, y);
        let south = self.sample(x, y + 1);
        (
            super::helpers::sign_lut_index(
                west.is_some_and(|idx| self.significant[idx]),
                west.map(|idx| self.sign_bits[idx]).unwrap_or(0),
                north.is_some_and(|idx| self.significant[idx]),
                north.map(|idx| self.sign_bits[idx]).unwrap_or(0),
                east.is_some_and(|idx| self.significant[idx]),
                east.map(|idx| self.sign_bits[idx]).unwrap_or(0),
                south.is_some_and(|idx| self.significant[idx]),
                south.map(|idx| self.sign_bits[idx]).unwrap_or(0),
            ),
            self.sign_bits[self.index(x as usize, y as usize)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::FlagGrid;
    use crate::tier1::consts::{T1_SIGMA_E, T1_SIGMA_N, T1_SIGMA_S, T1_SIGMA_W};

    #[test]
    fn neighbour_mask_tracks_significant_neighbours() {
        let mut grid = FlagGrid::new(3, 3);
        grid.mark_significant(1, 0, 0);
        grid.mark_significant(0, 1, 1);
        grid.mark_significant(2, 1, 0);
        grid.mark_significant(1, 2, 1);

        let mask = grid.neighbour_mask(1, 1);
        assert_eq!(mask & T1_SIGMA_N, T1_SIGMA_N);
        assert_eq!(mask & T1_SIGMA_W, T1_SIGMA_W);
        assert_eq!(mask & T1_SIGMA_E, T1_SIGMA_E);
        assert_eq!(mask & T1_SIGMA_S, T1_SIGMA_S);
    }

    #[test]
    fn stripe_is_clean_requires_no_context() {
        let mut grid = FlagGrid::new(3, 8);
        // Before any marking: all stripes should be clean
        assert!(grid.stripe_is_clean(1, 0, 4));
        assert!(grid.stripe_is_clean(1, 4, 4));

        // Marking a neighbor in an adjacent column should dirty the stripe
        grid.mark_significant(0, 2, 0);
        assert!(
            !grid.stripe_is_clean(1, 0, 4),
            "neighbour makes stripe dirty"
        );

        // Visited flag also makes stripe not clean
        let mut grid2 = FlagGrid::new(3, 8);
        grid2.mark_visited(1, 1);
        assert!(!grid2.stripe_is_clean(1, 0, 4));
    }
}
