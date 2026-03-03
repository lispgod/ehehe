/// Floor tile types for the game map.
#[derive(Clone, Debug, PartialEq)]
pub enum Floor {
    Gravel,
    Sand,
    Dirt,
    Grass,
    TallGrass,
    ScorchedEarth,
    WoodPlanks,
    /// Ground that is actively burning.
    Fire,
    /// Spilled water from a destroyed water trough.
    Water,
    /// Persistent sand/smoke cloud that blocks line of sight.
    /// Placed by sand throws and gun smoke; ticks down per game turn.
    SandCloud,
}

/// Props (obstacles/structures) placed on tiles.
#[derive(Clone, Debug, PartialEq)]
pub enum Props {
    Wall,
    Tree,
    Bush,
    Rock,
    DeadTree,
    Bench,
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

impl Props {
    /// Returns `true` if this prop blocks entity movement.
    /// Most solid objects block movement; low/open objects like fences and
    /// water troughs allow passage.
    pub fn blocks_movement(&self) -> bool {
        !matches!(self, Props::Fence | Props::WaterTrough)
    }

    /// Returns `true` if this prop blocks line-of-sight.
    /// Tall opaque objects block vision; short or transparent objects do not.
    pub fn blocks_vision(&self) -> bool {
        match self {
            // Short/open objects: you can see over/through them
            Props::Fence | Props::WaterTrough | Props::Bush
            | Props::Bench | Props::Chair | Props::HayBale
            | Props::Sign => false,
            _ => true,
        }
    }

    /// Returns `true` if this prop can be set on fire (destroyed by fire).
    pub fn is_flammable(&self) -> bool {
        matches!(
            self,
            Props::Wall | Props::Tree | Props::DeadTree | Props::Bush
            | Props::Barrel | Props::Crate | Props::Table
            | Props::Chair | Props::Piano | Props::Bench
            | Props::HayBale | Props::Sign | Props::Fence
        )
    }
}

impl std::fmt::Display for Props {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Props::Wall => write!(f, "Wall"),
            Props::Tree => write!(f, "Tree"),
            Props::Bush => write!(f, "Bush"),
            Props::Rock => write!(f, "Rock"),
            Props::DeadTree => write!(f, "Dead Tree"),
            Props::Bench => write!(f, "Bench"),
            Props::Barrel => write!(f, "Barrel"),
            Props::Crate => write!(f, "Crate"),
            Props::Cactus => write!(f, "Cactus"),
            Props::HitchingPost => write!(f, "Hitch Post"),
            Props::WaterTrough => write!(f, "Water Trgh"),
            Props::Fence => write!(f, "Fence"),
            Props::Table => write!(f, "Table"),
            Props::Chair => write!(f, "Chair"),
            Props::Piano => write!(f, "Piano"),
            Props::Sign => write!(f, "Sign"),
            Props::HayBale => write!(f, "Hay Bale"),
        }
    }
}
