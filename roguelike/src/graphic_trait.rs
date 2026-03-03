use crate::typeenums::{Floor, Props};
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
            Floor::ScorchedEarth => RatColor::Rgb(55, 30, 15),
            Floor::WoodPlanks => RatColor::Rgb(95, 70, 38),
            Floor::Fire => RatColor::Rgb(200, 60, 0),
            Floor::Water => RatColor::Rgb(40, 80, 140),
            Floor::SandCloud => RatColor::Rgb(170, 140, 90),
        }
    }
}

impl GraphicElement for Props {
    fn symbol(&self) -> String {
        match self {
            Props::Wall => "#".into(),
            Props::Tree => "T".into(),
            Props::Bush => "%".into(),
            Props::Rock => "o".into(),
            Props::DeadTree => "t".into(),
            Props::Bench => "H".into(),
            Props::Barrel => "0".into(),
            Props::Crate => "B".into(),
            Props::Cactus => "Y".into(),
            Props::HitchingPost => "F".into(),
            Props::WaterTrough => "U".into(),
            Props::Fence => "-".into(),
            Props::Table => "n".into(),
            Props::Chair => "h".into(),
            Props::Piano => "M".into(),
            Props::Sign => "]".into(),
            Props::HayBale => "&".into(),
        }
    }

    fn fg_color(&self) -> RatColor {
        match self {
            Props::Wall => RatColor::Rgb(120, 90, 45),
            Props::Tree => RatColor::Rgb(40, 100, 35),
            Props::Bush => RatColor::Rgb(55, 110, 40),
            Props::Rock => RatColor::Rgb(110, 105, 95),
            Props::DeadTree => RatColor::Rgb(90, 70, 45),
            Props::Bench => RatColor::Rgb(120, 80, 40),
            Props::Barrel => RatColor::Rgb(120, 80, 40),
            Props::Crate => RatColor::Rgb(130, 100, 50),
            Props::Cactus => RatColor::Rgb(50, 100, 40),
            Props::HitchingPost => RatColor::Rgb(105, 75, 38),
            Props::WaterTrough => RatColor::Rgb(65, 110, 145),
            Props::Fence => RatColor::Rgb(130, 100, 50),
            Props::Table => RatColor::Rgb(120, 80, 40),
            Props::Chair => RatColor::Rgb(105, 75, 38),
            Props::Piano => RatColor::Rgb(50, 45, 40),
            Props::Sign => RatColor::Rgb(150, 125, 70),
            Props::HayBale => RatColor::Rgb(200, 180, 80),
        }
    }

    fn bg_color(&self) -> RatColor {
        dim(self.fg_color(), 0.8)
    }
}
