/// Floor tile types for the game map.
#[derive(Clone, Debug, PartialEq)]
pub enum Floor {
    Gravel,
    Sand,
    Dirt,
    Grass,
    TallGrass,
    Flowers,
    Moss,
    Lava,
    ScorchedEarth,
}

/// Furniture (obstacles/structures) placed on tiles.
#[derive(Clone, Debug, PartialEq)]
pub enum Furniture {
    Wall,
    Tree,
    Bush,
    Rock,
    DeadTree,
}
