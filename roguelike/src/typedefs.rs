pub use ratatui::style::Color as RatColor;

/// A triple of (symbol, foreground color, background color) used to represent
/// a single cell in the roguelike grid.
pub type GraphicTriple = (String, RatColor, RatColor);

/// A 2D grid of GraphicTriples representing the renderable game screen.
/// Indexed as \[y\]\[x\] for row-major performance.
pub type RenderPacket = Vec<Vec<GraphicTriple>>;

pub type CoordinateUnit = i32;
pub type MyPoint = (CoordinateUnit, CoordinateUnit);

pub const SPAWN_X: CoordinateUnit = 60;
pub const SPAWN_Y: CoordinateUnit = 40;

/// Creates a 2D array of GraphicTriples initialized with spaces on a black background.
/// Indexed Y first for performance.
pub fn create_2d_array(render_width: usize, render_height: usize) -> RenderPacket {
    vec![vec![(" ".into(), RatColor::White, RatColor::Black); render_width]; render_height]
}
