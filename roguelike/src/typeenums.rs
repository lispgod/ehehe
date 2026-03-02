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
    WoodPlanks,
}

/// Furniture (obstacles/structures) placed on tiles.
#[derive(Clone, Debug, PartialEq)]
pub enum Furniture {
    Wall,
    Tree,
    Bush,
    Rock,
    DeadTree,
    Bench,
    LampPost,
    Barrel,
    Crate,
    Cactus,
    HitchingPost,
    WaterTrough,
    Fence,
    Table,
    Chair,
    Piano,
    Sign,
}
