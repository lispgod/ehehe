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

impl std::fmt::Display for Furniture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Furniture::Wall => write!(f, "Wall"),
            Furniture::Tree => write!(f, "Tree"),
            Furniture::Bush => write!(f, "Bush"),
            Furniture::Rock => write!(f, "Rock"),
            Furniture::DeadTree => write!(f, "Dead Tree"),
            Furniture::Bench => write!(f, "Bench"),
            Furniture::LampPost => write!(f, "Lamp Post"),
            Furniture::Barrel => write!(f, "Barrel"),
            Furniture::Crate => write!(f, "Crate"),
            Furniture::Cactus => write!(f, "Cactus"),
            Furniture::HitchingPost => write!(f, "Hitch Post"),
            Furniture::WaterTrough => write!(f, "Water Trgh"),
            Furniture::Fence => write!(f, "Fence"),
            Furniture::Table => write!(f, "Table"),
            Furniture::Chair => write!(f, "Chair"),
            Furniture::Piano => write!(f, "Piano"),
            Furniture::Sign => write!(f, "Sign"),
        }
    }
}
