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
            Floor::Gravel => ".".into(),
            Floor::Dirt => " ".into(),
            Floor::Grass => "\"".into(),
            Floor::Sand => ".".into(),
            Floor::TallGrass => ";".into(),
            Floor::Flowers => "*".into(),
            Floor::Moss => "~".into(),
        }
    }

    fn fg_color(&self) -> RatColor {
        match self {
            Floor::Sand => RatColor::Rgb(234, 208, 168),
            Floor::Dirt => RatColor::Rgb(107, 84, 40),
            Floor::Gravel => RatColor::Rgb(97, 84, 65),
            Floor::Grass => RatColor::Rgb(19, 109, 21),
            Floor::TallGrass => RatColor::Rgb(34, 139, 34),
            Floor::Flowers => RatColor::Rgb(218, 165, 32),
            Floor::Moss => RatColor::Rgb(50, 120, 50),
        }
    }

    fn bg_color(&self) -> RatColor {
        dim(self.fg_color(), 0.8)
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
        }
    }

    fn fg_color(&self) -> RatColor {
        match self {
            Furniture::Wall => RatColor::Rgb(139, 105, 20),
            Furniture::Tree => RatColor::Rgb(34, 139, 34),
            Furniture::Bush => RatColor::Rgb(60, 150, 40),
            Furniture::Rock => RatColor::Rgb(128, 128, 128),
            Furniture::DeadTree => RatColor::Rgb(100, 80, 50),
        }
    }

    fn bg_color(&self) -> RatColor {
        dim(self.fg_color(), 0.8)
    }
}
