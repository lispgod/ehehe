/// `GridVec` — a 2D integer vector forming an Abelian group under addition.
///
/// This is the fundamental coordinate type for the roguelike grid. It provides:
/// - **Algebraic structure**: `Add`, `Sub`, `Neg`, `AddAssign`, `SubAssign`
///   (the integers under addition form an Abelian group: associative,
///    commutative, identity element `ZERO`, and every element has an inverse).
/// - **Distance metrics**: Manhattan (L₁), Chebyshev (L∞), squared Euclidean.
/// - **Zero-cost abstraction**: `Copy` + `#[repr(C)]` + inline arithmetic.
///
/// Using a named struct instead of a raw tuple `(i32, i32)` gives us type
/// safety (cannot accidentally swap x/y with unrelated tuples), enables
/// method syntax, and makes the code self-documenting.
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use crate::typedefs::CoordinateUnit;

/// A 2D integer vector on the game grid.
///
/// Forms an **Abelian group** (ℤ², +) with:
/// - **Closure**: GridVec + GridVec → GridVec
/// - **Associativity**: (a + b) + c = a + (b + c)
/// - **Identity**: GridVec::ZERO (additive identity)
/// - **Inverse**: -v for every v (additive inverse via `Neg`)
/// - **Commutativity**: a + b = b + a
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(C)]
pub struct GridVec {
    pub x: CoordinateUnit,
    pub y: CoordinateUnit,
}

impl GridVec {
    /// The additive identity element: (0, 0).
    pub const ZERO: Self = Self { x: 0, y: 0 };

    /// Cardinal direction unit vectors.
    pub const NORTH: Self = Self { x: 0, y: 1 };
    pub const SOUTH: Self = Self { x: 0, y: -1 };
    pub const EAST: Self = Self { x: 1, y: 0 };
    pub const WEST: Self = Self { x: -1, y: 0 };

    /// Diagonal direction unit vectors.
    pub const NORTHEAST: Self = Self { x: 1, y: 1 };
    pub const NORTHWEST: Self = Self { x: -1, y: 1 };
    pub const SOUTHEAST: Self = Self { x: 1, y: -1 };
    pub const SOUTHWEST: Self = Self { x: -1, y: -1 };

    /// All 8 directional unit vectors (Moore neighbourhood).
    pub const DIRECTIONS_8: [Self; 8] = [
        Self::NORTH,
        Self::NORTHEAST,
        Self::EAST,
        Self::SOUTHEAST,
        Self::SOUTH,
        Self::SOUTHWEST,
        Self::WEST,
        Self::NORTHWEST,
    ];

    /// The 4 cardinal directions (Von Neumann neighbourhood).
    pub const DIRECTIONS_4: [Self; 4] = [Self::NORTH, Self::EAST, Self::SOUTH, Self::WEST];

    /// Construct a new `GridVec`.
    #[inline]
    pub const fn new(x: CoordinateUnit, y: CoordinateUnit) -> Self {
        Self { x, y }
    }

    /// Manhattan distance (L₁ norm): |Δx| + |Δy|.
    ///
    /// The natural distance metric for 4-connected grids.
    /// Counts the minimum number of cardinal moves to reach `other`.
    #[inline]
    pub fn manhattan_distance(self, other: Self) -> CoordinateUnit {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }

    /// Chebyshev distance (L∞ norm): max(|Δx|, |Δy|).
    ///
    /// The natural distance metric for 8-connected grids (king moves).
    /// Counts the minimum number of 8-directional moves to reach `other`.
    #[inline]
    pub fn chebyshev_distance(self, other: Self) -> CoordinateUnit {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }

    /// Squared Euclidean distance: Δx² + Δy².
    ///
    /// Avoids the `sqrt` call — use for comparisons where the exact
    /// distance is not needed, only the ordering (e.g., "is A closer
    /// than B?"). Correct because √ is monotonic.
    #[inline]
    pub fn distance_squared(self, other: Self) -> CoordinateUnit {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// Convert to `(f64, f64)` for floating-point calculations (noise, etc.).
    #[inline]
    pub fn as_f64(self) -> (f64, f64) {
        (self.x as f64, self.y as f64)
    }

    /// Dot product (inner product): a · b = aₓbₓ + aᵧbᵧ.
    ///
    /// The dot product captures the projection of one vector onto another.
    /// Useful for determining alignment:
    /// - Positive → vectors point in the same half-plane.
    /// - Zero → vectors are perpendicular (orthogonal).
    /// - Negative → vectors point in opposite half-planes.
    ///
    /// Also equals ‖a‖‖b‖cos θ, making it a key building block for
    /// angle calculations, line-of-sight checks, and lighting.
    #[inline]
    pub fn dot(self, other: Self) -> CoordinateUnit {
        self.x * other.x + self.y * other.y
    }

    /// 2D cross product (wedge product / perp-dot product):
    ///   a × b = aₓbᵧ − aᵧbₓ
    ///
    /// Geometrically, this is the **signed area** of the parallelogram
    /// spanned by `self` and `other`, and equals the determinant of the
    /// 2×2 matrix [self | other]:
    ///
    ///   det | aₓ  bₓ |
    ///       | aᵧ  bᵧ |  = aₓbᵧ − aᵧbₓ
    ///
    /// The sign determines orientation:
    /// - **Positive** → `other` is counter-clockwise from `self`.
    /// - **Zero** → vectors are collinear (parallel or anti-parallel).
    /// - **Negative** → `other` is clockwise from `self`.
    ///
    /// Essential for line-of-sight, convex hull, and turn-direction tests.
    #[inline]
    pub fn cross(self, other: Self) -> CoordinateUnit {
        self.x * other.y - self.y * other.x
    }

    /// Normalizes to a unit king-move step: each component clamped to {−1, 0, 1}.
    ///
    /// Projects any vector onto the Chebyshev unit ball, yielding the
    /// single-step 8-directional movement that best approximates the
    /// vector's direction. Equivalent to component-wise `signum`.
    #[inline]
    pub fn king_step(self) -> Self {
        Self {
            x: self.x.signum(),
            y: self.y.signum(),
        }
    }
}

// ─── Abelian group operations ───────────────────────────────────────────────

impl Add for GridVec {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for GridVec {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl Neg for GridVec {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl AddAssign for GridVec {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl SubAssign for GridVec {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

/// Scalar multiplication (ℤ-module structure): allows `grid_vec * scalar`.
impl Mul<CoordinateUnit> for GridVec {
    type Output = Self;
    #[inline]
    fn mul(self, scalar: CoordinateUnit) -> Self {
        Self {
            x: self.x * scalar,
            y: self.y * scalar,
        }
    }
}

// ─── Conversions ────────────────────────────────────────────────────────────

impl From<(CoordinateUnit, CoordinateUnit)> for GridVec {
    #[inline]
    fn from((x, y): (CoordinateUnit, CoordinateUnit)) -> Self {
        Self { x, y }
    }
}

impl From<GridVec> for (CoordinateUnit, CoordinateUnit) {
    #[inline]
    fn from(v: GridVec) -> Self {
        (v.x, v.y)
    }
}

// ─── Display ────────────────────────────────────────────────────────────────

impl std::fmt::Display for GridVec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additive_identity() {
        let v = GridVec::new(3, -7);
        assert_eq!(v + GridVec::ZERO, v);
        assert_eq!(GridVec::ZERO + v, v);
    }

    #[test]
    fn additive_inverse() {
        let v = GridVec::new(5, -3);
        assert_eq!(v + (-v), GridVec::ZERO);
    }

    #[test]
    fn commutativity() {
        let a = GridVec::new(2, 3);
        let b = GridVec::new(-1, 7);
        assert_eq!(a + b, b + a);
    }

    #[test]
    fn associativity() {
        let a = GridVec::new(1, 2);
        let b = GridVec::new(3, 4);
        let c = GridVec::new(5, 6);
        assert_eq!((a + b) + c, a + (b + c));
    }

    #[test]
    fn sub_is_add_neg() {
        let a = GridVec::new(10, 20);
        let b = GridVec::new(3, 7);
        assert_eq!(a - b, a + (-b));
    }

    #[test]
    fn manhattan_distance() {
        let a = GridVec::new(0, 0);
        let b = GridVec::new(3, 4);
        assert_eq!(a.manhattan_distance(b), 7);
        // Symmetric
        assert_eq!(b.manhattan_distance(a), 7);
    }

    #[test]
    fn chebyshev_distance() {
        let a = GridVec::new(0, 0);
        let b = GridVec::new(3, 4);
        assert_eq!(a.chebyshev_distance(b), 4);
        assert_eq!(b.chebyshev_distance(a), 4);
    }

    #[test]
    fn distance_squared() {
        let a = GridVec::new(0, 0);
        let b = GridVec::new(3, 4);
        assert_eq!(a.distance_squared(b), 25); // 3² + 4² = 25
    }

    #[test]
    fn scalar_multiplication() {
        let v = GridVec::new(2, -3);
        assert_eq!(v * 3, GridVec::new(6, -9));
        assert_eq!(v * 0, GridVec::ZERO);
    }

    #[test]
    fn tuple_round_trip() {
        let v = GridVec::new(42, -17);
        let tuple: (i32, i32) = v.into();
        let back: GridVec = tuple.into();
        assert_eq!(v, back);
    }

    #[test]
    fn directions_are_unit_length() {
        for dir in &GridVec::DIRECTIONS_4 {
            assert_eq!(dir.manhattan_distance(GridVec::ZERO), 1);
        }
    }

    // ─── Dot product tests ──────────────────────────────────────
    #[test]
    fn dot_product_orthogonal_is_zero() {
        let a = GridVec::EAST;
        let b = GridVec::NORTH;
        assert_eq!(a.dot(b), 0);
    }

    #[test]
    fn dot_product_parallel_positive() {
        let a = GridVec::new(3, 4);
        assert_eq!(a.dot(a), 25); // 3² + 4² = 25
    }

    #[test]
    fn dot_product_antiparallel_negative() {
        let a = GridVec::new(1, 0);
        let b = GridVec::new(-1, 0);
        assert!(a.dot(b) < 0);
    }

    #[test]
    fn dot_product_commutative() {
        let a = GridVec::new(2, 5);
        let b = GridVec::new(-3, 7);
        assert_eq!(a.dot(b), b.dot(a));
    }

    // ─── Cross product tests ────────────────────────────────────
    #[test]
    fn cross_product_collinear_is_zero() {
        let a = GridVec::new(2, 4);
        let b = GridVec::new(1, 2);
        assert_eq!(a.cross(b), 0);
    }

    #[test]
    fn cross_product_perpendicular() {
        let a = GridVec::EAST;
        let b = GridVec::NORTH;
        assert_eq!(a.cross(b), 1); // counter-clockwise
    }

    #[test]
    fn cross_product_anticommutative() {
        let a = GridVec::new(3, 1);
        let b = GridVec::new(-2, 5);
        assert_eq!(a.cross(b), -b.cross(a));
    }

    #[test]
    fn cross_product_self_is_zero() {
        let v = GridVec::new(7, -3);
        assert_eq!(v.cross(v), 0);
    }

    // ─── King step tests ────────────────────────────────────────
    #[test]
    fn king_step_normalizes_large_vector() {
        let v = GridVec::new(10, -5);
        assert_eq!(v.king_step(), GridVec::new(1, -1));
    }

    #[test]
    fn king_step_zero_stays_zero() {
        assert_eq!(GridVec::ZERO.king_step(), GridVec::ZERO);
    }

    #[test]
    fn king_step_unit_stays_same() {
        for dir in &GridVec::DIRECTIONS_8 {
            assert_eq!(dir.king_step(), *dir);
        }
    }
}
