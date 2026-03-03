use crate::typeenums::{Floor, Furniture};
use crate::typedefs::{GraphicTriple, RatColor};
use crate::voxel::dim;

/// Trait for game elements that can be rendered as a GraphicTriple.
pub trait GraphicElement {
    fn symbol(&self) -> String;
    fn fg_color(&self) -> RatColor;
    fn bg_color(&self) -> RatColor;

    fn to_graphic_triple(&self) -> GraphicTriple {
        (self.symbol(), self.fg_color(), self.bg_color())
    }
}

impl GraphicElement for Floor {
    fn symbol(&self) -> String {
        match self {
            Floor::Gravel => ",".into(),
            Floor::Dirt => " ".into(),
            Floor::Grass => " ".into(),
            Floor::Sand => " ".into(),
            Floor::TallGrass => ";".into(),
            Floor::Flowers => "*".into(),
            Floor::Moss => "~".into(),
            Floor::Lava => "~".into(),
            Floor::ScorchedEarth => ".".into(),
            Floor::WoodPlanks => " ".into(),
            Floor::Fire => "^".into(),
            Floor::Water => "~".into(),
            Floor::SandCloud => "*".into(),
        }
    }

    fn fg_color(&self) -> RatColor {
        match self {
            Floor::Sand => RatColor::Rgb(180, 160, 120),
            Floor::Dirt => RatColor::Rgb(100, 78, 42),
            Floor::Gravel => RatColor::Rgb(110, 95, 75),
            Floor::Grass => RatColor::Rgb(50, 80, 40),
            Floor::TallGrass => RatColor::Rgb(60, 100, 45),
            Floor::Flowers => RatColor::Rgb(180, 140, 50),
            Floor::Moss => RatColor::Rgb(50, 100, 50),
            Floor::Lava => RatColor::Rgb(255, 80, 0),
            Floor::ScorchedEarth => RatColor::Rgb(90, 40, 20),
            Floor::WoodPlanks => RatColor::Rgb(130, 95, 50),
            Floor::Fire => RatColor::Rgb(255, 140, 0),
            Floor::Water => RatColor::Rgb(80, 140, 200),
            Floor::SandCloud => RatColor::Rgb(210, 180, 120),
        }
    }

    fn bg_color(&self) -> RatColor {
        match self {
            Floor::Sand => RatColor::Rgb(160, 140, 100),
            Floor::Dirt => RatColor::Rgb(80, 62, 35),
            Floor::Gravel => RatColor::Rgb(85, 75, 60),
            Floor::Grass => RatColor::Rgb(35, 60, 28),
            Floor::TallGrass => RatColor::Rgb(40, 70, 30),
            Floor::Flowers => RatColor::Rgb(45, 65, 32),
            Floor::Moss => RatColor::Rgb(35, 70, 35),
            Floor::Lava => RatColor::Rgb(180, 50, 0),
            Floor::ScorchedEarth => RatColor::Rgb(55, 30, 15),
            Floor::WoodPlanks => RatColor::Rgb(95, 70, 38),
            Floor::Fire => RatColor::Rgb(200, 60, 0),
            Floor::Water => RatColor::Rgb(40, 80, 140),
            Floor::SandCloud => RatColor::Rgb(170, 140, 90),
        }
    }
}

impl GraphicElement for Furniture {
    fn symbol(&self) -> String {
        match self {
            Furniture::Wall => "#".into(),
            Furniture::Tree => "T".into(),
            Furniture::Bush => "%".into(),
            Furniture::Rock => "o".into(),
            Furniture::DeadTree => "t".into(),
            Furniture::Bench => "H".into(),
            Furniture::LampPost => "!".into(),
            Furniture::Barrel => "0".into(),
            Furniture::Crate => "B".into(),
            Furniture::Cactus => "Y".into(),
            Furniture::HitchingPost => "F".into(),
            Furniture::WaterTrough => "U".into(),
            Furniture::Fence => "-".into(),
            Furniture::Table => "n".into(),
            Furniture::Chair => "h".into(),
            Furniture::Piano => "M".into(),
            Furniture::Sign => "]".into(),
            Furniture::HayBale => "&".into(),
        }
    }

    fn fg_color(&self) -> RatColor {
        match self {
            Furniture::Wall => RatColor::Rgb(120, 90, 45),
            Furniture::Tree => RatColor::Rgb(40, 100, 35),
            Furniture::Bush => RatColor::Rgb(55, 110, 40),
            Furniture::Rock => RatColor::Rgb(110, 105, 95),
            Furniture::DeadTree => RatColor::Rgb(90, 70, 45),
            Furniture::Bench => RatColor::Rgb(120, 80, 40),
            Furniture::LampPost => RatColor::Rgb(170, 140, 50),
            Furniture::Barrel => RatColor::Rgb(120, 80, 40),
            Furniture::Crate => RatColor::Rgb(130, 100, 50),
            Furniture::Cactus => RatColor::Rgb(50, 100, 40),
            Furniture::HitchingPost => RatColor::Rgb(105, 75, 38),
            Furniture::WaterTrough => RatColor::Rgb(65, 110, 145),
            Furniture::Fence => RatColor::Rgb(130, 100, 50),
            Furniture::Table => RatColor::Rgb(120, 80, 40),
            Furniture::Chair => RatColor::Rgb(105, 75, 38),
            Furniture::Piano => RatColor::Rgb(50, 45, 40),
            Furniture::Sign => RatColor::Rgb(150, 125, 70),
            Furniture::HayBale => RatColor::Rgb(200, 180, 80),
        }
    }

    fn bg_color(&self) -> RatColor {
        dim(self.fg_color(), 0.8)
    }
}
