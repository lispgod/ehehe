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
    /// Ground that is actively burning.
    Fire,
    /// Spilled water from a destroyed water trough.
    Water,
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
    /// A bale of hay — blocks movement but not vision, and is flammable.
    HayBale,
}

impl Furniture {
    /// Returns `true` if this furniture blocks entity movement.
    /// Most solid objects block movement; low/open objects like fences and
    /// water troughs allow passage.
    pub fn blocks_movement(&self) -> bool {
        !matches!(self, Furniture::Fence | Furniture::WaterTrough)
    }

    /// Returns `true` if this furniture blocks line-of-sight.
    /// Tall opaque objects block vision; short or transparent objects do not.
    pub fn blocks_vision(&self) -> bool {
        match self {
            // Short/open objects: you can see over/through them
            Furniture::Fence | Furniture::WaterTrough | Furniture::Bush
            | Furniture::Bench | Furniture::Chair | Furniture::HayBale
            | Furniture::Sign => false,
            _ => true,
        }
    }

    /// Returns `true` if this furniture can be set on fire (destroyed by fire).
    pub fn is_flammable(&self) -> bool {
        matches!(
            self,
            Furniture::Tree | Furniture::DeadTree | Furniture::Bush
            | Furniture::Barrel | Furniture::Crate | Furniture::Table
            | Furniture::Chair | Furniture::Piano | Furniture::Bench
            | Furniture::HayBale | Furniture::Sign | Furniture::Fence
        )
    }
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
            Furniture::HayBale => write!(f, "Hay Bale"),
        }
    }
}
