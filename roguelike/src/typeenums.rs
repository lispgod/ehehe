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
    /// Shallow river water — passable but slow.
    ShallowWater,
    /// Deep river water — passable but very slow.
    DeepWater,
    /// Sandy beach along the river banks.
    Beach,
    /// Bridge over the river — normal movement speed.
    Bridge,
    /// Packed-dirt sidewalk flanking carriage roads.
    Sidewalk,
    /// Accessible rooftop tile — reached via interior stairs or ladders.
    Rooftop,
    /// Open plaza/market square — exposed killzone.
    Plaza,
    /// Narrow alley between buildings — ambush terrain.
    Alley,
}

/// Construction material for building walls.
/// Affects breachability, flammability, and tactical value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WallMaterial {
    /// Sun-dried clay brick — moderate breach cost, low flammability.
    Adobe,
    /// Wooden plank/log construction — cheap to breach, highly flammable.
    Timber,
    /// Cut stone or masonry — very expensive to breach, negligible flammability.
    Stone,
}

impl WallMaterial {
    /// Returns the relative cost to breach this wall type.
    /// Lower values mean easier/faster breaching.
    #[inline]
    pub fn breach_cost(&self) -> u32 {
        match self {
            WallMaterial::Timber => 1,
            WallMaterial::Adobe => 2,
            WallMaterial::Stone => 5,
        }
    }

    /// Returns the flammability rating `[0.0, 1.0]` for this material.
    /// Higher values mean more susceptible to fire.
    #[inline]
    pub fn flammability(&self) -> f64 {
        match self {
            WallMaterial::Timber => 0.8,
            WallMaterial::Adobe => 0.2,
            WallMaterial::Stone => 0.0,
        }
    }
}

/// Building height tier — determines verticality and rooftop advantage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HeightTier {
    /// Ground level only — no rooftop access.
    SingleStory,
    /// Two floors — rooftop tiles overlook adjacent streets.
    DoubleStory,
    /// Tall structure (bell tower, watchtower, grain loft) —
    /// extended sight lines covering entire street segments.
    Tower,
}

impl HeightTier {
    /// Returns the sight-line bonus (in tiles) for entities on this rooftop.
    #[inline]
    pub fn sight_bonus(&self) -> i32 {
        match self {
            HeightTier::SingleStory => 0,
            HeightTier::DoubleStory => 4,
            HeightTier::Tower => 10,
        }
    }
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
    /// The victory goal at the top-right corner of the map.
    VictoryGoal,
}

impl Props {
    /// Returns `true` if this prop is any kind of wall.
    #[inline]
    pub fn is_wall(&self) -> bool {
        matches!(self, Props::Wall)
    }

    /// Returns `true` if this prop blocks entity movement.
    /// Most solid objects block movement; low/open objects like fences and
    /// water troughs allow passage.
    pub fn blocks_movement(&self) -> bool {
        !matches!(self, Props::Fence | Props::WaterTrough | Props::VictoryGoal)
    }

    /// Returns `true` if this prop blocks line-of-sight.
    /// Tall opaque objects block vision; short or transparent objects do not.
    pub fn blocks_vision(&self) -> bool {
        match self {
            // Short/open objects: you can see over/through them
            Props::Fence | Props::WaterTrough | Props::Bush
            | Props::Bench | Props::Chair | Props::HayBale
            | Props::Sign | Props::VictoryGoal => false,
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
            Props::VictoryGoal => write!(f, "Gold Cache"),
        }
    }
}
