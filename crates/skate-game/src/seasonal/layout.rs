//! Not retail (user 10-19): what each season stands where in the Super Ultra Mega Park, scene by scene. Authored from
//! a survey of the park's collision; place.rs snaps every entry to the real surfaces and drops what does not land.
//! Points are world metres. Not tested in play.

use super::Season;
use Kind::*;
use Palette::*;
use Snap::*;
use Zone::*;

/// The park's seven scenes, each dressed as one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Zone {
    WestGate,
    TopDeck,
    UpperTerrace,
    MainFloor,
    DeepPit,
    SouthRim,
    SouthGate,
}

pub(crate) const ZONES: [Zone; 7] =
    [Zone::WestGate, Zone::TopDeck, Zone::UpperTerrace, Zone::MainFloor, Zone::DeepPit, Zone::SouthRim, Zone::SouthGate];

/// Every piece either season stands about the park.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    House,
    Witch,
    Spider,
    /// A giant spider sat on a deck's edge, its front legs hanging down the face.
    DrapedSpider,
    /// Not retail (user 10-20): a spider on a car's roof, every leg gripping over the roof's edges.
    CarSpider,
    Ghost,
    HangingGhost,
    Graveyard,
    Hay,
    Cauldron,
    Broom,
    Skeleton,
    EggSac,
    LightPole,
    PumpkinTower,
    BatBanner,
    CryptLantern,
    Fence,
    Pumpkin,
    Mist,
    Uplight,
    BigTree,
    Snowman,
    CandyCane,
    Nutcracker,
    Wreath,
    Presents,
    Gingerbread,
    Sleigh,
    Reindeer,
    SnowGlobe,
    Sled,
    Lantern,
}

impl Kind {
    /// The room a ground piece needs clear about it (radius, height), at scale 1.
    pub(crate) fn room(self) -> (f32, f32) {
        match self {
            Kind::House => (2.6, 5.6),
            Kind::Spider | Kind::DrapedSpider | Kind::CarSpider => (1.9, 1.8),
            Kind::Witch => (1.0, 3.0),
            Kind::Ghost => (1.0, 2.6),
            Kind::Graveyard => (2.3, 1.2),
            Kind::Hay => (1.4, 1.6),
            Kind::Cauldron => (0.8, 1.4),
            Kind::Broom => (0.4, 1.6),
            Kind::Skeleton => (0.6, 1.9),
            Kind::EggSac => (0.5, 0.8),
            Kind::LightPole => (1.2, 9.),
            Kind::PumpkinTower => (1.1, 3.2),
            Kind::BatBanner => (0.6, 4.),
            Kind::CryptLantern => (0.3, 1.),
            Kind::Fence => (0.3, 1.2),
            Kind::Pumpkin => (0.4, 0.6),
            Kind::Mist => (0.5, 0.5),
            Kind::Uplight => (0.3, 0.4),
            Kind::BigTree => (6.8, 15.),
            Kind::Snowman => (0.8, 2.2),
            Kind::CandyCane => (0.3, 1.6),
            Kind::Nutcracker => (0.8, 3.2),
            Kind::Wreath => (0.5, 1.),
            Kind::Presents => (1., 1.),
            Kind::Gingerbread => (2.2, 3.4),
            Kind::Sleigh => (1.8, 1.8),
            Kind::Reindeer => (1., 2.2),
            Kind::SnowGlobe => (1.4, 3.),
            Kind::Sled => (0.6, 0.5),
            Kind::Lantern => (0.3, 1.),
            Kind::HangingGhost => (0.6, 1.8),
        }
    }
}

/// How an entry finds its surface below its authored point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Snap {
    /// Level open ground, with its kind's room clear.
    Ground,
    /// The first surface below, any slope (a beam's top, a ledge).
    Top,
    /// Exactly as authored, kept only while a surface is near (hung from a beam, on a wall).
    Hung,
}

/// One piece: `yaw` radians from +Z toward +X (the way its front looks).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    pub zone: Zone,
    pub kind: Kind,
    pub at: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
    pub snap: Snap,
}

/// How a strand hangs between its points.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Style {
    /// Even swags between supports.
    Swag,
    /// Pulled straight along the points.
    Taut,
    /// Straight, with strands of bulbs hanging down from it.
    Icicles,
    /// Wound round from the foot centre (`points[0]`) to the top centre (`points[1]`), `radius` out at the foot and
    /// `top` at the top.
    Spiral { radius: f32, top: f32, turns: f32 },
}

/// A strand's bulb colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Palette {
    Halloween,
    Warm,
    Multi,
    Ice,
}

/// A strand of lights along `points`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Route {
    pub zone: Zone,
    pub points: &'static [[f32; 3]],
    pub style: Style,
    pub palette: Palette,
    pub garland: bool,
}

/// One kind repeated every `spacing` along `points`, each facing the path's right-hand side (looking from
/// `points[0]` to `points[1]`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Row {
    pub zone: Zone,
    pub kind: Kind,
    pub points: &'static [[f32; 3]],
    pub spacing: f32,
    pub snap: Snap,
    pub scale: f32,
}

/// A giant web strung between the anchors of `frame`, with a giant spider at its hub when `spider`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WebSpan {
    pub zone: Zone,
    pub frame: &'static [[f32; 3]],
    pub spider: bool,
}

/// A season's whole park.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout<'a> {
    pub pieces: &'a [Piece],
    pub routes: &'a [Route],
    pub rows: &'a [Row],
    pub webs: &'a [WebSpan],
    /// Where each flock of bats circles.
    pub flocks: &'a [[f32; 3]],
    /// Bats hanging upside down from the beams.
    pub roosts: &'a [[f32; 3]],
}

impl Layout<'_> {
    #[cfg(test)]
    pub(crate) const EMPTY: Self = Layout { pieces: &[], routes: &[], rows: &[], webs: &[], flocks: &[], roosts: &[] };
}

const fn p(zone: Zone, kind: Kind, at: [f32; 3], yaw: f32, scale: f32, snap: Snap) -> Piece {
    Piece { zone, kind, at, yaw, scale, snap }
}

const fn row(zone: Zone, kind: Kind, points: &'static [[f32; 3]], spacing: f32, scale: f32, snap: Snap) -> Row {
    Row { zone, kind, points, spacing, snap, scale }
}

const fn swag(zone: Zone, palette: Palette, garland: bool, points: &'static [[f32; 3]]) -> Route {
    Route { zone, points, style: Style::Swag, palette, garland }
}

const fn taut(zone: Zone, palette: Palette, garland: bool, points: &'static [[f32; 3]]) -> Route {
    Route { zone, points, style: Style::Taut, palette, garland }
}

/// A straight strand with bulbs hanging down from it.
const fn icicles(zone: Zone, palette: Palette, garland: bool, points: &'static [[f32; 3]]) -> Route {
    Route { zone, points, style: Style::Icicles, palette, garland }
}

/// A strand wound up a tower: `points` are the foot and top centres.
const fn spiral(zone: Zone, palette: Palette, points: &'static [[f32; 3]], radius: f32, top: f32, turns: f32) -> Route {
    Route { zone, points, style: Style::Spiral { radius, top, turns }, palette, garland: false }
}

const fn web(zone: Zone, spider: bool, frame: &'static [[f32; 3]]) -> WebSpan {
    WebSpan { zone, frame, spider }
}

// The Halloween park, scene by scene as the spec's "Halloween (spooky-fun)" list has it, from the survey of the installed
// map's collision (each entry checked against it: nothing dropped). Not retail (user 10-20): "a lot more", every piece
// off the lines people skate: on tops of ramps and walls, in dead-end corners, on faces and overhead.
const HW_PIECES: &[Piece] = &[
    // WestGate.
    p(WestGate, DrapedSpider, [161.94, 163.2, -706.89], -0.37, 2.2, Top),
    p(WestGate, Uplight, [154.44, 163.0, -710.38], -0.37, 1.0, Top),
    p(WestGate, Uplight, [169.84, 163.0, -704.42], -0.37, 1.0, Top),
    p(WestGate, Hay, [149.0, 153.0, -703.5], -0.37, 1.0, Ground),
    p(WestGate, PumpkinTower, [147.0, 152.5, -705.2], -0.37, 1.0, Ground),
    p(WestGate, Ghost, [148.5, 160.3, -717.6], -0.37, 1.15, Ground),
    p(WestGate, Ghost, [153.5, 160.3, -717.6], -0.37, 1.15, Ground),
    p(WestGate, Hay, [171.0, 152.4, -697.0], -0.37, 1.0, Ground),
    p(WestGate, PumpkinTower, [173.3, 152.5, -694.6], -0.37, 1.0, Ground),
    p(WestGate, Ghost, [178.8, 160.3, -706.6], -0.37, 1.15, Ground),
    p(WestGate, Ghost, [183.8, 160.3, -706.6], -0.37, 1.15, Ground),
    p(WestGate, BatBanner, [158.4, 151.5, -727.14], -1.94, 1.0, Top),
    p(WestGate, Uplight, [149.8, 152.5, -707.6], -0.37, 1.0, Top),
    p(WestGate, BatBanner, [178.18, 151.5, -719.48], 1.2, 1.0, Top),
    p(WestGate, Uplight, [169.4, 152.5, -700.1], -0.37, 1.0, Top),
    p(WestGate, Graveyard, [144.6, 160.3, -720.4], -0.37, 1.0, Ground),
    p(WestGate, Skeleton, [151.0, 160.3, -720.8], -0.6, 1.0, Ground),
    p(WestGate, CryptLantern, [156.0, 160.3, -719.5], 0.0, 1.0, Ground),
    p(WestGate, Hay, [175.3, 160.3, -709.6], -0.37, 1.0, Ground),
    p(WestGate, PumpkinTower, [182.4, 160.3, -709.2], -0.37, 1.0, Ground),
    p(WestGate, HangingGhost, [159.2, 157.9, -711.8], -0.37, 1.0, Hung),
    p(WestGate, HangingGhost, [162.6, 157.9, -710.5], -0.37, 1.0, Hung),
    p(WestGate, HangingGhost, [166.0, 157.9, -709.2], -0.37, 1.0, Hung),
    p(WestGate, HangingGhost, [161.11, 157.95, -714.45], -0.37, 1.0, Hung),
    p(WestGate, HangingGhost, [164.38, 157.95, -713.19], -0.37, 1.0, Hung),
    p(WestGate, HangingGhost, [167.64, 157.95, -711.92], -0.37, 1.0, Hung),
    p(WestGate, Ghost, [156.83, 151.5, -728.34], 2.77, 1.15, Top),
    p(WestGate, Ghost, [180.16, 151.5, -719.31], 2.77, 1.15, Top),
    p(WestGate, Uplight, [158.78, 151.5, -728.12], 2.77, 1.0, Top),
    p(WestGate, Uplight, [178.56, 151.5, -720.46], 2.77, 1.0, Top),
    // TopDeck.
    p(TopDeck, BatBanner, [343.45, 141.0, -735.1], -1.57, 1.0, Top),
    p(TopDeck, BatBanner, [343.35, 141.0, -722.9], -1.57, 1.0, Top),
    p(TopDeck, BatBanner, [336.05, 141.0, -743.5], 1.57, 1.0, Top),
    p(TopDeck, BatBanner, [315.2, 141.0, -782.5], -1.57, 1.0, Top),
    p(TopDeck, BatBanner, [257.85, 143.5, -775.1], 1.57, 1.0, Top),
    p(TopDeck, Uplight, [344.6, 141.0, -735.1], 1.57, 1.0, Top),
    p(TopDeck, Uplight, [344.5, 141.0, -722.9], 1.57, 1.0, Top),
    p(TopDeck, Ghost, [277.7, 140.9, -781.8], 0.0, 1.15, Ground),
    p(TopDeck, Ghost, [287.8, 141.2, -783.4], 0.0, 1.15, Top),
    p(TopDeck, Ghost, [260.0, 143.6, -783.0], 0.0, 1.15, Top),
    p(TopDeck, Ghost, [260.0, 143.0, -767.5], 0.0, 1.15, Top),
    p(TopDeck, Hay, [269.6, 141.5, -781.6], 0.79, 1.0, Top),
    p(TopDeck, Hay, [260.3, 143.2, -778.6], 0.0, 1.0, Top),
    p(TopDeck, Hay, [323.45, 140.6, -757.66], -0.89, 1.0, Ground),
    p(TopDeck, Uplight, [318.38, 141.0, -766.42], -0.89, 1.0, Top),
    p(TopDeck, Uplight, [322.15, 141.0, -761.75], -0.89, 1.0, Top),
    p(TopDeck, Uplight, [327.16, 141.0, -755.52], -0.89, 1.0, Top),
    p(TopDeck, Uplight, [332.18, 141.0, -749.28], -0.89, 1.0, Top),
    p(TopDeck, DrapedSpider, [331.96, 147.6, -752.96], -0.89, 1.2, Top),
    p(TopDeck, BatBanner, [315.18, 141.0, -767.69], -0.89, 1.0, Top),
    p(TopDeck, CarSpider, [290.45, 142.4, -775.2], 0.0, 0.6, Top),
    p(TopDeck, Pumpkin, [344.0, 151.0, -735.1], 1.57, 2.4, Top),
    p(TopDeck, Pumpkin, [343.9, 151.0, -722.9], 1.57, 2.4, Top),
    p(TopDeck, CryptLantern, [293.0, 141.2, -783.4], 0.0, 1.2, Top),
    p(TopDeck, Graveyard, [254.8, 143.6, -771.5], 1.57, 1.0, Top),
    p(TopDeck, Skeleton, [341.2, 140.6, -742.3], 2.4, 1.0, Top),
    p(TopDeck, Skeleton, [258.5, 143.0, -785.5], 1.2, 1.0, Top),
    p(TopDeck, Skeleton, [252.0, 143.5, -777.5], 1.9, 1.0, Top),
    p(TopDeck, CryptLantern, [259.8, 142.5, -784.8], 0.0, 1.0, Top),
    p(TopDeck, Ghost, [250.0, 143.5, -782.5], 1.57, 1.15, Top),
    p(TopDeck, PumpkinTower, [259.6, 142.5, -772.0], 1.2, 1.0, Top),
    p(TopDeck, Mist, [254.5, 143.5, -782.0], 0.0, 1.0, Top),
    p(TopDeck, Spider, [358.6, 143.8, -741.2], 2.6, 1.3, Top),
    p(TopDeck, Skeleton, [355.0, 143.8, -743.5], 2.8, 1.0, Top),
    p(TopDeck, PumpkinTower, [347.5, 143.0, -750.5], 2.4, 1.0, Top),
    p(TopDeck, CryptLantern, [362.5, 143.8, -741.0], 0.0, 1.0, Top),
    p(TopDeck, Hay, [351.5, 143.5, -745.0], 2.4, 1.0, Top),
    // UpperTerrace.
    p(UpperTerrace, House, [271.0, 132.5, -719.0], 0.25, 1.0, Ground),
    p(UpperTerrace, Witch, [266.8, 132.5, -714.4], 1.36, 1.0, Ground),
    p(UpperTerrace, Witch, [274.3, 132.5, -714.9], -1.24, 1.0, Ground),
    p(UpperTerrace, Witch, [268.3, 132.5, -710.6], 2.51, 1.0, Ground),
    p(UpperTerrace, Witch, [273.2, 132.5, -710.4], -2.44, 1.0, Ground),
    p(UpperTerrace, Cauldron, [270.5, 132.5, -713.6], 0.0, 1.0, Ground),
    p(UpperTerrace, Mist, [270.5, 132.5, -713.6], 0.0, 0.8, Top),
    p(UpperTerrace, Broom, [265.0, 132.5, -720.3], 0.3, 1.0, Ground),
    p(UpperTerrace, Broom, [277.2, 132.5, -720.4], -0.2, 1.0, Ground),
    p(UpperTerrace, Broom, [278.6, 132.5, -716.8], -0.6, 1.0, Ground),
    p(UpperTerrace, CryptLantern, [268.0, 132.5, -715.9], 0.0, 1.0, Ground),
    p(UpperTerrace, CryptLantern, [274.0, 132.5, -715.9], 0.0, 1.0, Ground),
    p(UpperTerrace, Hay, [262.3, 132.5, -718.6], 0.3, 1.0, Ground),
    p(UpperTerrace, Hay, [280.2, 132.5, -712.0], -0.8, 1.0, Ground),
    p(UpperTerrace, Skeleton, [264.6, 132.5, -706.4], 0.8, 1.0, Top),
    p(UpperTerrace, PumpkinTower, [282.6, 132.5, -700.3], -0.6, 1.0, Ground),
    p(UpperTerrace, Uplight, [265.0, 132.5, -722.0], 3.14, 1.0, Top),
    p(UpperTerrace, Uplight, [275.5, 132.5, -720.2], 3.14, 1.0, Top),
    p(UpperTerrace, Skeleton, [258.6, 132.5, -719.6], 0.9, 1.0, Ground),
    p(UpperTerrace, Skeleton, [277.8, 132.5, -706.0], -0.5, 1.0, Ground),
    p(UpperTerrace, Mist, [259.6, 132.5, -714.6], 0.0, 1.0, Top),
    p(UpperTerrace, Ghost, [281.6, 132.5, -705.0], -0.8, 1.15, Ground),
    p(UpperTerrace, Pumpkin, [276.2, 132.5, -712.2], 0.0, 1.4, Ground),
    p(UpperTerrace, Pumpkin, [264.8, 132.5, -712.0], 0.0, 1.4, Ground),
    p(UpperTerrace, Uplight, [262.5, 132.5, -722.9], 0.0, 1.0, Top),
    p(UpperTerrace, Uplight, [277.0, 132.5, -721.2], 0.0, 1.0, Top),
    p(UpperTerrace, Uplight, [287.0, 132.5, -712.9], 0.0, 1.0, Top),
    p(UpperTerrace, EggSac, [260.4, 132.5, -722.6], 0.8, 1.2, Top),
    // MainFloor.
    p(MainFloor, LightPole, [355.0, 107.0, -606.5], 0.0, 1.3, Top),
    p(MainFloor, PumpkinTower, [348.7, 105.6, -612.6], 0.79, 1.0, Top),
    p(MainFloor, PumpkinTower, [357.4, 106.0, -613.0], -0.79, 1.0, Top),
    p(MainFloor, Spider, [361.5, 107.0, -607.5], -1.2, 1.3, Top),
    p(MainFloor, Hay, [370.5, 107.0, -603.5], 0.4, 0.9, Top),
    p(MainFloor, PumpkinTower, [376.5, 106.5, -612.0], -0.4, 1.0, Top),
    p(MainFloor, Spider, [389.0, 123.0, -604.0], -0.9, 1.6, Top),
    p(MainFloor, Ghost, [385.0, 122.5, -598.0], -0.7, 1.3, Top),
    p(MainFloor, CryptLantern, [352.6, 105.0, -612.6], 0.0, 1.0, Top),
    p(MainFloor, Mist, [355.0, 107.0, -606.5], 0.0, 1.2, Top),
    p(MainFloor, Skeleton, [366.0, 107.0, -602.0], -0.6, 1.0, Top),
    p(MainFloor, Uplight, [343.4, 105.5, -609.6], -1.4, 1.0, Top),
    p(MainFloor, Uplight, [365.6, 107.5, -609.0], 1.57, 1.0, Top),
    p(MainFloor, Skeleton, [381.0, 106.0, -614.0], -0.5, 1.0, Top),
    p(MainFloor, PumpkinTower, [394.0, 123.5, -604.0], -0.9, 1.0, Top),
    p(MainFloor, CryptLantern, [386.0, 122.5, -606.0], 0.0, 1.0, Top),
    p(MainFloor, CryptLantern, [377.8, 102.5, -620.2], 0.0, 1.0, Top),
    p(MainFloor, CryptLantern, [392.8, 101.5, -619.2], 0.0, 1.0, Top),
    p(MainFloor, Mist, [362.9, 106.0, -616.9], 0.0, 1.0, Top),
    p(MainFloor, EggSac, [393.5, 106.0, -663.5], 0.4, 1.2, Top),
    p(MainFloor, EggSac, [386.6, 106.0, -663.4], -0.6, 1.2, Top),
    // DeepPit.
    p(DeepPit, EggSac, [222.5, 89.0, -701.5], 0.0, 1.2, Ground),
    p(DeepPit, EggSac, [224.5, 89.0, -702.8], 0.7, 1.2, Ground),
    p(DeepPit, EggSac, [226.8, 89.0, -702.0], 1.4, 1.2, Ground),
    p(DeepPit, EggSac, [229.5, 89.0, -701.2], 2.1, 1.2, Ground),
    p(DeepPit, EggSac, [231.8, 89.0, -702.4], 2.8, 1.2, Ground),
    p(DeepPit, EggSac, [220.5, 89.0, -699.8], 3.5, 1.2, Ground),
    p(DeepPit, EggSac, [233.0, 89.0, -700.0], 4.2, 1.2, Ground),
    p(DeepPit, EggSac, [218.5, 89.0, -701.0], 4.9, 1.2, Ground),
    p(DeepPit, Spider, [222.0, 89.2, -696.0], 0.0, 1.4, Top),
    p(DeepPit, Spider, [216.0, 89.2, -697.5], 0.0, 1.4, Top),
    p(DeepPit, Uplight, [230.0, 89.0, -696.5], 0.0, 1.0, Top),
    p(DeepPit, Uplight, [240.0, 89.0, -696.5], 0.0, 1.0, Top),
    p(DeepPit, Uplight, [250.0, 89.0, -696.5], 0.0, 1.0, Top),
    p(DeepPit, Graveyard, [209.5, 89.0, -585.0], 1.57, 1.0, Ground),
    p(DeepPit, Skeleton, [208.0, 89.0, -591.5], 1.57, 1.0, Ground),
    p(DeepPit, Skeleton, [211.0, 89.0, -578.5], 1.2, 1.0, Ground),
    p(DeepPit, CryptLantern, [207.2, 89.0, -596.4], 0.0, 1.0, Ground),
    p(DeepPit, CryptLantern, [207.2, 89.0, -573.6], 0.0, 1.0, Ground),
    p(DeepPit, Ghost, [211.8, 89.0, -592.0], 1.57, 1.15, Ground),
    p(DeepPit, Mist, [209.5, 89.0, -585.0], 0.0, 1.0, Top),
    p(DeepPit, Spider, [209.6, 89.0, -681.0], 1.2, 1.2, Top),
    p(DeepPit, Hay, [213.0, 89.0, -684.6], 0.8, 1.0, Top),
    p(DeepPit, Skeleton, [226.0, 89.0, -697.4], 0.0, 1.0, Ground),
    p(DeepPit, Skeleton, [237.5, 89.0, -694.8], 0.0, 1.0, Ground),
    p(DeepPit, Pumpkin, [214.9, 89.0, -597.0], 0.0, 1.2, Ground),
    p(DeepPit, Ghost, [268.2, 89.0, -684.0], -1.57, 1.15, Ground),
    p(DeepPit, Ghost, [268.2, 89.0, -678.0], -1.57, 1.15, Ground),
    p(DeepPit, CryptLantern, [207.0, 89.0, -590.0], 0.0, 1.0, Top),
    p(DeepPit, CryptLantern, [207.0, 89.0, -578.0], 0.0, 1.0, Top),
    // SouthRim.
    p(SouthRim, Graveyard, [362.5, 139.5, -575.5], 3.14, 1.0, Top),
    p(SouthRim, Graveyard, [362.5, 139.5, -568.5], 3.14, 1.0, Top),
    p(SouthRim, Graveyard, [362.0, 140.5, -549.0], 3.14, 1.0, Top),
    p(SouthRim, Skeleton, [360.0, 139.5, -572.0], 3.14, 1.0, Top),
    p(SouthRim, Skeleton, [365.0, 139.5, -564.0], 3.14, 1.0, Top),
    p(SouthRim, Skeleton, [360.5, 140.5, -553.0], 3.14, 1.0, Top),
    p(SouthRim, Skeleton, [364.8, 139.5, -579.0], 3.14, 1.0, Top),
    p(SouthRim, CryptLantern, [358.6, 139.5, -581.0], 3.14, 1.0, Top),
    p(SouthRim, CryptLantern, [366.4, 139.5, -581.0], 3.14, 1.0, Top),
    p(SouthRim, CryptLantern, [358.6, 141.0, -546.0], 3.14, 1.0, Top),
    p(SouthRim, CryptLantern, [366.4, 141.0, -546.0], 3.14, 1.0, Top),
    p(SouthRim, Mist, [362.0, 139.5, -572.0], 0.0, 1.0, Top),
    p(SouthRim, Mist, [362.0, 140.0, -562.0], 0.0, 1.0, Top),
    p(SouthRim, Mist, [362.5, 140.5, -551.0], 0.0, 1.0, Top),
    p(SouthRim, Graveyard, [353.0, 139.0, -566.0], 3.14, 1.0, Top),
    p(SouthRim, Skeleton, [354.5, 139.5, -558.0], 2.8, 1.0, Top),
    p(SouthRim, Skeleton, [352.0, 139.5, -552.0], 3.4, 1.0, Top),
    p(SouthRim, Ghost, [352.0, 139.0, -573.0], 3.14, 1.15, Top),
    p(SouthRim, CryptLantern, [355.5, 140.0, -546.0], 0.0, 1.0, Top),
    p(SouthRim, CryptLantern, [355.5, 139.0, -578.5], 0.0, 1.0, Top),
    p(SouthRim, Mist, [353.0, 139.0, -566.0], 0.0, 1.0, Top),
    p(SouthRim, Skeleton, [369.5, 140.0, -556.0], 3.0, 1.0, Top),
    p(SouthRim, Ghost, [370.5, 140.5, -548.0], 3.14, 1.15, Top),
    p(SouthRim, Uplight, [372.6, 140.5, -576.0], -1.57, 1.0, Top),
    p(SouthRim, Uplight, [373.6, 141.0, -563.0], -1.57, 1.0, Top),
    p(SouthRim, Spider, [379.5, 162.5, -568.0], 3.14, 1.6, Top),
    p(SouthRim, Hay, [369.2, 139.5, -575.5], -1.57, 1.0, Top),
    p(SouthRim, Witch, [355.0, 140.5, -543.0], 0.6, 1.0, Top),
    p(SouthRim, Cauldron, [357.4, 140.5, -542.2], 0.0, 1.0, Top),
    p(SouthRim, Mist, [357.4, 140.5, -542.2], 0.0, 1.0, Top),
    p(SouthRim, Skeleton, [369.5, 140.0, -560.0], -1.57, 1.0, Top),
    p(SouthRim, Uplight, [361.0, 140.0, -558.6], 3.14, 1.0, Top),
    p(SouthRim, Uplight, [350.2, 139.5, -560.0], 0.0, 1.0, Top),
    // SouthGate.
    p(SouthGate, HangingGhost, [266.0, 88.0, -538.0], 0.0, 1.0, Hung),
    p(SouthGate, HangingGhost, [270.5, 88.0, -538.0], 0.0, 1.0, Hung),
    p(SouthGate, HangingGhost, [275.0, 88.0, -538.0], 0.0, 1.0, Hung),
    p(SouthGate, HangingGhost, [279.5, 88.0, -538.0], 0.0, 1.0, Hung),
    p(SouthGate, HangingGhost, [284.0, 88.0, -538.0], 0.0, 1.0, Hung),
    p(SouthGate, Uplight, [262.9, 77.5, -533.0], 1.57, 1.0, Top),
    p(SouthGate, Uplight, [288.6, 77.5, -533.0], -1.57, 1.0, Top),
    p(SouthGate, DrapedSpider, [276.0, 103.2, -537.0], 0.0, 2.0, Top),
    p(SouthGate, Ghost, [257.5, 86.0, -521.0], 1.57, 1.15, Top),
    p(SouthGate, Ghost, [257.5, 86.0, -528.5], 1.57, 1.15, Top),
    p(SouthGate, Skeleton, [252.5, 88.5, -517.0], 1.57, 1.0, Top),
    p(SouthGate, Uplight, [259.5, 83.0, -533.0], 0.0, 1.0, Top),
    p(SouthGate, Hay, [295.2, 79.5, -514.5], -1.2, 0.8, Top),
    p(SouthGate, PumpkinTower, [295.5, 80.0, -521.0], -1.57, 0.9, Top),
    p(SouthGate, Ghost, [294.5, 79.5, -525.5], -1.57, 1.15, Top),
    p(SouthGate, Uplight, [290.6, 80.5, -534.6], 0.0, 1.0, Top),
    p(SouthGate, Graveyard, [279.0, 108.5, -565.0], 0.0, 0.85, Top),
    p(SouthGate, Ghost, [276.0, 108.5, -558.0], 0.3, 1.15, Top),
    p(SouthGate, Ghost, [285.2, 110.5, -559.9], -0.3, 1.15, Top),
    p(SouthGate, PumpkinTower, [282.85, 110.5, -553.35], 0.0, 1.0, Top),
    p(SouthGate, Skeleton, [276.0, 109.5, -556.0], 0.0, 1.0, Top),
    p(SouthGate, Mist, [279.0, 108.5, -565.0], 0.0, 1.0, Top),
    p(SouthGate, Ghost, [248.5, 90.5, -517.0], 1.57, 1.15, Top),
    p(SouthGate, Ghost, [250.0, 87.5, -507.5], 1.57, 1.15, Top),
];

const HW_ROWS: &[Row] = &[
    // WestGate.
    row(WestGate, Pumpkin, &[[152.53, 163.0, -710.85], [158.69, 163.0, -708.47]], 1.45, 1.6, Top),
    row(WestGate, Pumpkin, &[[165.41, 163.0, -705.87], [171.56, 163.0, -703.49]], 1.45, 1.6, Top),
    row(WestGate, Pumpkin, &[[147.0, 160.3, -715.5], [155.0, 160.3, -715.5]], 1.6, 1.2, Ground),
    row(WestGate, Pumpkin, &[[177.2, 160.3, -703.6], [185.2, 160.3, -703.6]], 1.6, 1.2, Ground),
    row(WestGate, Pumpkin, &[[154.09, 151.5, -728.75], [155.2, 151.5, -729.93]], 1.3, 1.3, Top),
    row(WestGate, Pumpkin, &[[182.46, 151.5, -717.78], [182.44, 151.5, -719.39]], 1.3, 1.3, Top),
    // TopDeck.
    row(TopDeck, Pumpkin, &[[266.4, 141.6, -761.0], [266.4, 141.6, -784.1], [282.6, 141.2, -784.1]], 1.6, 1.4, Top),
    row(TopDeck, Pumpkin, &[[316.59, 140.6, -767.54], [322.91, 140.6, -759.92]], 1.5, 1.2, Ground),
    row(TopDeck, Pumpkin, &[[325.54, 140.6, -756.65], [334.67, 140.6, -745.39]], 1.5, 1.2, Ground),
    row(TopDeck, Pumpkin, &[[275.0, 142.08, -776.6], [275.0, 142.08, -777.4]], 0.8, 0.9, Top),
    row(TopDeck, Pumpkin, &[[280.55, 142.05, -776.6], [280.55, 142.05, -777.4]], 0.8, 0.9, Top),
    row(TopDeck, Pumpkin, &[[285.35, 141.92, -776.4], [285.35, 141.92, -777.2]], 0.8, 0.9, Top),
    row(TopDeck, Pumpkin, &[[256.2, 143.8, -784.0], [256.2, 143.6, -778.0]], 2.0, 1.3, Top),
    row(TopDeck, Pumpkin, &[[253.4, 144.0, -784.0], [253.4, 143.8, -776.0]], 2.0, 1.3, Top),
    row(TopDeck, CryptLantern, &[[262.6, 143.0, -785.0], [262.6, 142.6, -765.0]], 4.0, 1.0, Top),
    row(TopDeck, Pumpkin, &[[339.6, 140.6, -744.6], [341.5, 140.6, -744.0], [342.4, 140.6, -743.2]], 1.2, 1.2, Top),
    row(TopDeck, Pumpkin, &[[264.3, 143.5, -771.5], [264.3, 143.5, -760.5]], 2.4, 1.2, Top),
    row(TopDeck, Pumpkin, &[[268.0, 143.5, -784.95], [294.4, 143.5, -784.95]], 4.4, 1.2, Top),
    row(TopDeck, Pumpkin, &[[250.0, 144.0, -780.0], [250.0, 144.0, -773.6]], 1.6, 1.2, Top),
    // UpperTerrace.
    row(UpperTerrace, Pumpkin, &[
        [262.4, 132.5, -714.0], [262.4, 132.5, -709.5], [278.9, 132.5, -709.5],
    ], 1.5, 1.2, Ground),
    row(UpperTerrace, CryptLantern, &[
        [256.6, 132.5, -720.5], [257.6, 132.5, -712.8], [264.0, 132.5, -706.8], [270.6, 132.5, -701.4],
    ], 3.0, 1.0, Ground),
    row(UpperTerrace, Pumpkin, &[[287.0, 132.5, -712.6], [305.0, 132.5, -712.6]], 1.6, 1.2, Ground),
    // MainFloor.
    row(MainFloor, Ghost, &[[360.0, 106.5, -616.0], [371.0, 106.5, -616.0]], 2.75, 1.3, Top),
    row(MainFloor, Ghost, &[[345.8, 106.0, -611.0], [345.8, 106.0, -603.0]], 4.0, 1.3, Top),
    row(MainFloor, Ghost, &[[349.5, 107.5, -600.5], [361.5, 107.5, -600.5]], 6.0, 1.3, Top),
    row(MainFloor, Pumpkin, &[
        [357.0, 107.0, -606.5], [356.41, 107.0, -607.91], [355.0, 107.0, -608.5], [353.59, 107.0, -607.91],
        [353.0, 107.0, -606.5], [353.59, 107.0, -605.09], [355.0, 107.0, -604.5], [356.41, 107.0, -605.09],
        [357.0, 107.0, -606.5],
    ], 1.25, 1.2, Top),
    row(MainFloor, Pumpkin, &[[374.0, 108.5, -611.0], [374.0, 108.5, -605.0]], 1.5, 1.2, Top),
    row(MainFloor, Pumpkin, &[[367.0, 104.5, -661.9], [393.0, 104.5, -661.9]], 2.2, 1.2, Top),
    // DeepPit.
    row(DeepPit, Pumpkin, &[[206.52, 94.2, -672.1], [206.52, 94.2, -664.0]], 1.6, 0.75, Top),
    row(DeepPit, Pumpkin, &[[206.52, 94.2, -652.0], [206.52, 94.2, -644.0]], 1.6, 0.75, Top),
    row(DeepPit, Pumpkin, &[[236.0, 89.0, -693.6], [244.0, 89.0, -693.6]], 2.0, 1.3, Ground),
    row(DeepPit, Pumpkin, &[[248.0, 89.0, -693.6], [254.0, 89.0, -693.6]], 2.0, 1.3, Ground),
    row(DeepPit, Pumpkin, &[[206.52, 94.2, -630.4], [206.52, 94.2, -622.4]], 1.6, 0.75, Top),
    row(DeepPit, Pumpkin, &[[206.52, 94.2, -610.9], [206.52, 94.2, -602.9]], 1.6, 0.75, Top),
    row(DeepPit, Pumpkin, &[[214.9, 89.0, -591.25], [214.9, 89.0, -574.0]], 2.88, 1.2, Ground),
    // SouthRim.
    row(SouthRim, Fence, &[[358.6, 140.5, -581.0], [358.6, 141.5, -545.5]], 4.0, 1.0, Top),
    row(SouthRim, Fence, &[[366.4, 141.5, -545.5], [366.4, 140.5, -581.0]], 4.0, 1.0, Top),
    row(SouthRim, Fence, &[[366.4, 140.5, -581.0], [358.6, 140.5, -581.0]], 3.9, 1.0, Top),
    row(SouthRim, Pumpkin, &[[365.5, 139.0, -579.2], [359.5, 139.0, -579.2]], 1.5, 1.2, Top),
    row(SouthRim, Pumpkin, &[[356.6, 139.0, -578.0], [356.6, 140.0, -548.0]], 2.0, 1.2, Top),
    // SouthGate.
    row(SouthGate, CryptLantern, &[[255.5, 88.0, -515.0], [255.5, 88.0, -529.5]], 2.4, 1.0, Top),
    row(SouthGate, CryptLantern, &[[291.2, 80.5, -530.8], [291.2, 80.5, -517.0]], 3.0, 1.0, Top),
    row(SouthGate, Pumpkin, &[[254.0, 89.0, -515.0], [254.0, 89.0, -527.0]], 1.8, 1.2, Top),
    row(SouthGate, Pumpkin, &[[292.8, 81.0, -532.0], [292.8, 81.0, -515.8]], 1.8, 1.2, Top),
    row(SouthGate, Pumpkin, &[[267.0, 103.2, -537.6], [271.0, 103.2, -537.6]], 2.0, 1.3, Top),
    row(SouthGate, Pumpkin, &[[281.0, 103.2, -537.6], [285.0, 103.2, -537.6]], 2.0, 1.3, Top),
    row(SouthGate, Pumpkin, &[[249.3, 89.0, -514.5], [249.3, 87.8, -510.0]], 1.5, 1.2, Top),
];

const HW_ROUTES: &[Route] = &[
    // WestGate.
    swag(WestGate, Halloween, true, &[
        [151.08, 159.55, -710.16], [152.95, 158.5, -709.44], [155.84, 158.5, -708.32], [158.73, 158.5, -707.2],
        [161.63, 158.5, -706.08], [164.52, 158.5, -704.96], [167.41, 158.5, -703.84], [170.3, 158.5, -702.72],
        [172.17, 159.55, -702.0],
    ]),
    swag(WestGate, Halloween, false, &[
        [151.08, 162.35, -710.16], [154.6, 162.35, -708.8], [158.11, 162.35, -707.44], [161.63, 162.35, -706.08],
        [165.14, 162.35, -704.72], [168.65, 162.35, -703.36], [172.17, 162.35, -702.0],
    ]),
    taut(WestGate, Halloween, false, &[
        [152.57, 162.35, -711.77], [155.83, 162.35, -710.51], [159.1, 162.35, -709.24], [162.36, 162.35, -707.98],
        [165.63, 162.35, -706.72], [168.89, 162.35, -705.45], [172.16, 162.35, -704.19],
    ]),
    taut(WestGate, Halloween, false, &[
        [158.94, 158.05, -713.68], [161.38, 158.05, -712.74], [163.84, 158.05, -711.79], [166.29, 158.05, -710.84],
        [168.73, 158.05, -709.89],
    ]),
    swag(WestGate, Halloween, true, &[
        [159.6, 158.6, -720.24], [162.86, 158.6, -718.98], [166.13, 158.6, -717.71], [169.39, 158.6, -716.45],
        [172.66, 158.6, -715.19],
    ]),
    // TopDeck.
    swag(TopDeck, Halloween, true, &[
        [316.5, 139.2, -722.1], [324.3, 139.2, -722.1], [332.1, 139.2, -722.1], [339.9, 139.2, -722.1],
    ]),
    spiral(TopDeck, Halloween, &[[344.0, 140.5, -735.1], [344.0, 151.0, -735.1]], 0.85, 0.85, 7.0),
    spiral(TopDeck, Halloween, &[[343.9, 140.5, -722.9], [343.9, 151.0, -722.9]], 0.85, 0.85, 7.0),
    taut(TopDeck, Halloween, false, &[
        [316.79, 146.6, -768.22], [323.21, 146.6, -760.55], [329.63, 146.6, -752.88], [336.04, 146.6, -745.21],
    ]),
    swag(TopDeck, Halloween, true, &[
        [316.61, 146.3, -768.07], [319.82, 146.3, -764.24], [323.02, 146.3, -760.4], [326.23, 146.3, -756.56],
        [329.44, 146.3, -752.73], [332.65, 146.3, -748.89], [335.85, 146.3, -745.06],
    ]),
    taut(TopDeck, Halloween, false, &[[316.88, 151.35, -768.03], [320.48, 151.35, -763.74], [324.08, 151.35, -759.46]]),
    swag(TopDeck, Halloween, true, &[
        [316.1, 146.3, -784.6], [316.1, 146.3, -780.0], [316.1, 146.3, -775.4], [316.1, 146.3, -770.8],
    ]),
    taut(TopDeck, Halloween, false, &[
        [273.94, 141.0, -777.3], [273.94, 141.0, -775.04], [275.0, 141.0, -775.04], [276.06, 141.0, -775.04],
        [276.06, 141.0, -777.3], [276.06, 141.0, -779.56], [275.0, 141.0, -779.56], [273.94, 141.0, -779.56],
        [273.94, 141.0, -777.3],
    ]),
    taut(TopDeck, Halloween, false, &[
        [279.54, 141.0, -777.2], [279.54, 141.0, -775.04], [280.55, 141.0, -775.04], [281.56, 141.0, -775.04],
        [281.56, 141.0, -777.2], [281.56, 141.0, -779.36], [280.55, 141.0, -779.36], [279.54, 141.0, -779.36],
        [279.54, 141.0, -777.2],
    ]),
    taut(TopDeck, Halloween, false, &[
        [284.34, 141.0, -776.95], [284.34, 141.0, -774.84], [285.35, 141.0, -774.84], [286.36, 141.0, -774.84],
        [286.36, 141.0, -776.95], [286.36, 141.0, -779.06], [285.35, 141.0, -779.06], [284.34, 141.0, -779.06],
        [284.34, 141.0, -776.95],
    ]),
    taut(TopDeck, Halloween, false, &[
        [289.34, 141.0, -776.75], [289.34, 141.0, -774.24], [290.45, 141.0, -774.24], [291.56, 141.0, -774.24],
        [291.56, 141.0, -776.75], [291.56, 141.0, -779.26], [290.45, 141.0, -779.26], [289.34, 141.0, -779.26],
        [289.34, 141.0, -776.75],
    ]),
    swag(TopDeck, Halloween, true, &[
        [266.0, 142.6, -784.35], [274.0, 142.6, -784.35], [282.0, 142.6, -784.35], [290.0, 142.6, -784.35],
        [298.0, 142.6, -784.35], [306.0, 142.6, -784.35], [314.0, 142.6, -784.35],
    ]),
    swag(TopDeck, Halloween, true, &[
        [265.24, 142.5, -783.8], [265.04, 142.5, -776.2], [264.83, 142.5, -768.6], [264.63, 142.5, -761.0],
    ]),
    swag(TopDeck, Halloween, true, &[
        [316.69, 150.9, -767.88], [318.49, 150.9, -765.74], [320.29, 150.9, -763.59], [322.09, 150.9, -761.45],
        [323.9, 150.9, -759.31],
    ]),
    taut(TopDeck, Halloween, false, &[[316.37, 146.6, -784.6], [316.37, 146.6, -777.7], [316.37, 146.6, -770.8]]),
    taut(TopDeck, Halloween, false, &[
        [316.5, 139.95, -722.27], [322.35, 139.95, -722.27], [328.2, 139.95, -722.27], [334.05, 139.95, -722.27],
        [339.9, 139.95, -722.27],
    ]),
    // UpperTerrace.
    taut(UpperTerrace, Halloween, false, &[
        [260.8, 132.37, -709.0], [260.8, 132.37, -722.0], [280.6, 132.37, -720.6], [280.6, 132.37, -709.0],
    ]),
    taut(UpperTerrace, Halloween, false, &[[271.0, 139.0, -722.25], [260.8, 132.37, -709.0]]),
    taut(UpperTerrace, Halloween, false, &[[271.0, 139.0, -722.25], [280.6, 132.37, -709.0]]),
    swag(UpperTerrace, Halloween, true, &[
        [258.4, 139.6, -725.28], [264.0, 139.6, -723.67], [270.0, 139.6, -722.38], [276.0, 139.6, -721.63],
        [281.6, 139.6, -721.31],
    ]),
    swag(UpperTerrace, Halloween, true, &[[284.6, 140.6, -713.2], [289.3, 140.6, -713.2], [294.0, 140.6, -713.2]]),
    // MainFloor.
    taut(MainFloor, Halloween, false, &[
        [351.0, 122.0, -613.1], [354.63, 117.27, -607.11], [355.66, 117.27, -606.22], [364.5, 107.3, -602.5],
    ]),
    taut(MainFloor, Halloween, false, &[
        [366.6, 112.0, -608.5], [355.7, 117.27, -606.62], [354.3, 117.27, -606.66], [346.5, 105.5, -608.5],
    ]),
    taut(MainFloor, Halloween, false, &[
        [340.5, 124.0, -607.5], [354.29, 117.27, -606.55], [355.52, 117.27, -606.99], [363.5, 106.0, -614.5],
    ]),
    taut(MainFloor, Halloween, false, &[
        [347.5, 106.7, -601.0], [354.42, 117.27, -606.08], [355.71, 117.27, -606.43], [365.0, 107.0, -605.5],
    ]),
    spiral(MainFloor, Halloween, &[[350.55, 105.3, -613.08], [350.55, 123.8, -613.08]], 0.5, 0.5, 12.0),
    // DeepPit.
    swag(DeepPit, Halloween, true, &[
        [205.9, 89.0, -688.0], [205.9, 89.0, -660.0], [205.9, 89.0, -632.0], [205.9, 89.0, -604.0],
        [205.9, 89.0, -576.0],
    ]),
    swag(DeepPit, Halloween, false, &[
        [207.15, 87.0, -672.0], [207.15, 87.0, -654.0], [207.15, 87.0, -636.0], [207.15, 87.0, -618.0],
        [207.15, 87.0, -600.0],
    ]),
    swag(DeepPit, Halloween, true, &[
        [222.0, 88.8, -692.9], [231.0, 88.8, -692.9], [240.0, 88.8, -692.9], [249.0, 88.8, -692.9],
        [258.0, 88.8, -692.9],
    ]),
    // SouthRim.
    swag(SouthRim, Halloween, false, &[[349.15, 145.5, -558.8], [362.2, 145.5, -557.4]]),
    taut(SouthRim, Halloween, false, &[
        [359.1, 138.28, -578.0], [359.1, 138.52, -574.0], [359.1, 138.74, -570.0], [359.1, 138.99, -566.0],
        [359.1, 139.24, -562.0], [359.1, 139.49, -558.0], [359.1, 139.74, -554.0], [359.1, 139.99, -550.0],
        [359.1, 140.24, -546.0],
    ]),
    taut(SouthRim, Halloween, false, &[
        [365.9, 138.24, -578.0], [365.9, 138.46, -574.0], [365.9, 138.68, -570.0], [365.9, 138.89, -566.0],
        [365.9, 139.11, -562.0], [365.9, 139.33, -558.0], [365.9, 139.56, -554.0], [365.9, 139.78, -550.0],
        [365.9, 140.0, -546.0],
    ]),
    // SouthGate.
    swag(SouthGate, Halloween, true, &[
        [266.0, 100.5, -536.2], [271.0, 100.5, -536.2], [276.0, 100.5, -536.2], [281.0, 100.5, -536.2],
        [286.0, 100.5, -536.2],
    ]),
    swag(SouthGate, Halloween, false, &[
        [266.0, 92.0, -536.0], [271.0, 92.0, -536.0], [276.0, 92.0, -536.0], [281.0, 92.0, -536.0],
        [286.0, 92.0, -536.0],
    ]),
    taut(SouthGate, Halloween, false, &[[266.3, 102.45, -536.3], [269.05, 102.45, -536.3], [271.8, 102.45, -536.3]]),
    taut(SouthGate, Halloween, false, &[[280.2, 102.45, -536.3], [282.6, 102.45, -536.3], [285.0, 102.45, -536.3]]),
    swag(SouthGate, Halloween, true, &[
        [266.0, 100.5, -539.5], [271.0, 100.5, -539.5], [276.0, 100.5, -539.5], [281.0, 100.5, -539.5],
        [286.0, 100.5, -539.5],
    ]),
    swag(SouthGate, Halloween, false, &[
        [266.0, 92.0, -539.5], [271.0, 92.0, -539.5], [276.0, 92.0, -539.5], [281.0, 92.0, -539.5],
        [286.0, 92.0, -539.5],
    ]),
];

const HW_WEBS: &[WebSpan] = &[
    // WestGate.
    web(WestGate, false, &[[155.22, 166.0, -710.72], [158.86, 162.75, -708.35], [155.0, 162.75, -710.16]]),
    web(WestGate, false, &[[154.09, 165.8, -711.16], [151.47, 162.75, -710.89], [153.87, 162.75, -710.6]]),
    web(WestGate, false, &[[169.49, 166.0, -705.2], [165.2, 162.75, -705.9], [169.28, 162.75, -704.64]]),
    web(WestGate, false, &[[170.62, 165.8, -704.76], [172.37, 162.75, -702.8], [170.41, 162.75, -704.2]]),
    web(WestGate, false, &[[147.0, 153.5, -710.0], [151.0, 155.5, -710.0], [149.0, 159.3, -713.5]]),
    web(WestGate, true, &[[172.0, 154.4, -701.0], [176.0, 156.5, -701.0], [175.5, 159.2, -703.2]]),
    web(WestGate, true, &[[157.42, 157.95, -712.66], [159.58, 157.95, -718.26], [157.57, 155.0, -715.82]]),
    web(WestGate, false, &[[169.17, 157.95, -708.11], [171.34, 157.95, -713.71], [171.19, 155.0, -710.55]]),
    // TopDeck.
    web(TopDeck, true, &[[344.0, 150.0, -734.4], [344.0, 150.0, -723.55], [344.0, 144.6, -729.0]]),
    web(TopDeck, false, &[[336.1, 150.5, -742.6], [343.4, 150.5, -735.4], [339.8, 145.4, -739.0]]),
    // UpperTerrace.
    web(UpperTerrace, true, &[[266.0, 139.0, -723.1], [276.0, 139.0, -721.55], [271.0, 135.6, -719.6]]),
    web(UpperTerrace, true, &[[286.5, 139.5, -713.2], [293.5, 139.5, -713.2], [290.0, 135.5, -711.8]]),
    web(UpperTerrace, true, &[[282.7, 139.5, -721.1], [282.7, 134.5, -716.0], [275.0, 136.5, -721.55]]),
    // MainFloor.
    web(MainFloor, true, &[[351.5, 118.5, -694.8], [360.5, 118.5, -694.8], [356.0, 108.5, -691.9]]),
    web(MainFloor, false, &[[348.8, 124.3, -616.7], [340.5, 124.0, -607.5], [344.5, 113.0, -612.0]]),
    // DeepPit.
    web(DeepPit, true, &[[207.0, 93.6, -672.1], [207.0, 93.6, -664.0], [207.2, 88.95, -668.05]]),
    web(DeepPit, false, &[[207.0, 93.6, -652.0], [207.0, 93.6, -644.0], [207.2, 88.95, -648.0]]),
    web(DeepPit, true, &[[206.5, 93.6, -643.3], [206.5, 93.6, -631.1], [205.0, 91.0, -637.2]]),
    web(DeepPit, false, &[[207.0, 93.6, -630.4], [207.0, 93.6, -622.4], [207.2, 88.95, -626.4]]),
    web(DeepPit, true, &[[207.0, 93.6, -610.9], [207.0, 93.6, -602.9], [207.2, 88.95, -606.9]]),
    web(DeepPit, true, &[[235.05, 95.0, -693.9], [245.95, 95.0, -693.9], [241.0, 88.75, -693.9]]),
    web(DeepPit, false, &[[246.0, 96.0, -697.9], [257.0, 93.8, -698.5], [251.0, 88.75, -693.4]]),
    web(DeepPit, true, &[[225.0, 99.0, -705.7], [231.0, 99.0, -703.5], [228.0, 90.5, -699.5]]),
    // SouthRim.
    web(SouthRim, true, &[[349.15, 143.5, -558.8], [362.2, 143.5, -557.4], [355.7, 140.4, -558.1]]),
    // SouthGate.
    web(SouthGate, false, &[[262.85, 87.6, -536.9], [268.5, 88.0, -536.9], [262.85, 81.0, -536.9]]),
    web(SouthGate, true, &[[288.7, 87.6, -536.9], [283.0, 88.0, -536.9], [288.7, 81.0, -536.9]]),
    web(SouthGate, true, &[[262.93, 87.6, -538.9], [268.5, 88.9, -539.5], [262.93, 81.0, -538.9]]),
];

const HW_FLOCKS: &[[f32; 3]] = &[
    [159.5, 170.0, -700.59], [150.05, 168.0, -714.98], [300.0, 153.0, -752.0], [268.0, 151.0, -735.0],
    [271.0, 143.0, -713.0], [392.0, 140.0, -728.0], [356.0, 112.0, -640.0], [385.0, 110.0, -660.0],
    [420.0, 112.0, -650.0], [240.0, 92.0, -650.0], [235.0, 90.0, -600.0], [215.0, 100.0, -630.0],
    [362.0, 150.0, -562.0], [395.0, 155.0, -575.0], [277.0, 98.0, -530.0], [262.0, 95.0, -548.0],
    [330.31, 157.0, -761.9], [180.0, 168.0, -708.0], [290.0, 152.0, -768.0], [283.0, 151.0, -779.0],
    [382.0, 114.0, -640.0], [345.0, 112.0, -640.0], [250.0, 92.0, -688.0], [276.0, 100.0, -552.0],
];

const HW_ROOSTS: &[[f32; 3]] = &[
    [157.22, 157.9, -708.55], [159.09, 157.9, -707.83], [160.95, 157.9, -707.11], [162.82, 157.9, -706.39],
    [164.69, 157.9, -705.67], [166.55, 157.9, -704.94], [156.87, 157.9, -710.41], [158.73, 157.9, -709.69],
    [160.6, 157.9, -708.96], [162.46, 157.9, -708.24], [164.33, 157.9, -707.52], [166.2, 157.9, -706.8],
    [168.06, 157.9, -706.08], [154.89, 157.9, -709.46], [168.88, 157.9, -704.04], [264.5, 88.0, -537.0],
    [268.0, 88.0, -537.0], [272.5, 88.0, -537.0], [277.0, 88.0, -537.0], [281.5, 88.0, -537.0],
    [286.5, 88.0, -537.0], [264.5, 88.0, -539.4], [268.0, 88.0, -539.4], [272.5, 88.0, -539.4],
    [277.0, 88.0, -539.4], [281.5, 88.0, -539.4], [286.5, 88.0, -539.4], [264.5, 88.2, -540.6],
    [268.0, 88.2, -540.6], [272.5, 88.2, -540.6], [277.0, 88.2, -540.6], [281.5, 88.2, -540.6],
    [286.5, 88.2, -540.6], [160.5, 157.9, -714.3], [163.5, 157.9, -713.1], [166.5, 157.9, -711.9],
    [169.0, 157.9, -710.9], [161.19, 157.95, -718.82], [163.53, 157.95, -717.92], [165.86, 157.95, -717.01],
    [168.19, 157.95, -716.11], [170.52, 157.95, -715.21], [262.0, 140.3, -724.0], [266.0, 140.3, -723.0],
    [270.0, 140.3, -722.3],
];

pub(crate) static HALLOWEEN: Layout<'static> = Layout {
    pieces: HW_PIECES,
    routes: HW_ROUTES,
    rows: HW_ROWS,
    webs: HW_WEBS,
    flocks: HW_FLOCKS,
    roosts: HW_ROOSTS,
};

// The Christmas park, scene by scene as the spec's Christmas list has it. Not retail (user 10-20): the Halloween park's
// lessons applied, on its audited spots: each Halloween piece's Christmas counterpart where it stands and no bigger, so
// every piece stays off the lines people skate; then set pieces with no Halloween spot (the wreaths, the presents on
// the cars, the south span's icicles). From the survey of the installed map's collision (each entry checked against it:
// nothing dropped).
const XM_PIECES: &[Piece] = &[
    // WestGate.
    p(WestGate, Uplight, [154.44, 163.0, -710.38], -0.37, 1.0, Top),
    p(WestGate, Uplight, [169.84, 163.0, -704.42], -0.37, 1.0, Top),
    p(WestGate, Presents, [149.0, 153.0, -703.5], -0.37, 1.0, Ground),
    p(WestGate, SnowGlobe, [147.0, 152.5, -705.2], -0.37, 0.79, Ground),
    p(WestGate, Nutcracker, [148.5, 160.3, -717.6], -0.37, 1.0, Ground),
    p(WestGate, Snowman, [153.5, 160.3, -717.6], -0.37, 1.15, Ground),
    p(WestGate, Presents, [171.0, 152.4, -697.0], -0.37, 1.0, Ground),
    p(WestGate, SnowGlobe, [173.3, 152.5, -694.6], -0.37, 0.79, Ground),
    p(WestGate, Snowman, [178.8, 160.3, -706.6], -0.37, 1.15, Ground),
    p(WestGate, Nutcracker, [183.8, 160.3, -706.6], -0.37, 1.0, Ground),
    p(WestGate, CandyCane, [158.4, 151.5, -727.14], -1.94, 1.5, Top),
    p(WestGate, Uplight, [149.8, 152.5, -707.6], -0.37, 1.0, Top),
    p(WestGate, CandyCane, [178.18, 151.5, -719.48], 1.2, 1.5, Top),
    p(WestGate, Uplight, [169.4, 152.5, -700.1], -0.37, 1.0, Top),
    // TopDeck.
    p(TopDeck, CandyCane, [343.45, 141.0, -735.1], -1.57, 1.5, Top),
    p(TopDeck, CandyCane, [343.35, 141.0, -722.9], -1.57, 1.5, Top),
    p(TopDeck, CandyCane, [336.05, 141.0, -743.5], 1.57, 1.5, Top),
    p(TopDeck, CandyCane, [315.2, 141.0, -782.5], -1.57, 1.5, Top),
    p(TopDeck, CandyCane, [257.85, 143.5, -775.1], 1.57, 1.5, Top),
    p(TopDeck, Uplight, [344.6, 141.0, -735.1], 1.57, 1.0, Top),
    p(TopDeck, Uplight, [344.5, 141.0, -722.9], 1.57, 1.0, Top),
    p(TopDeck, Snowman, [277.7, 140.9, -781.8], 0.0, 1.15, Ground),
    p(TopDeck, Snowman, [287.8, 141.2, -783.4], 0.0, 1.15, Top),
    p(TopDeck, Snowman, [260.0, 143.6, -783.0], 0.0, 1.15, Top),
    p(TopDeck, Snowman, [260.0, 143.0, -767.5], 0.0, 1.15, Top),
    p(TopDeck, Presents, [269.6, 141.5, -781.6], 0.79, 1.0, Top),
    p(TopDeck, Presents, [260.3, 143.2, -778.6], 0.0, 1.0, Top),
    // UpperTerrace.
    p(UpperTerrace, Gingerbread, [271.0, 132.5, -719.0], 0.25, 1.0, Ground),
    p(UpperTerrace, Presents, [266.8, 132.5, -714.4], 1.36, 0.8, Ground),
    p(UpperTerrace, Presents, [274.6, 132.5, -714.2], -1.24, 0.8, Ground),
    p(UpperTerrace, Reindeer, [269.3, 132.5, -711.0], 0.0, 0.9, Ground),
    p(UpperTerrace, Reindeer, [271.7, 132.5, -711.0], 0.0, 0.9, Ground),
    p(UpperTerrace, Sleigh, [270.5, 132.5, -714.0], 0.0, 1.0, Ground),
    p(UpperTerrace, CandyCane, [265.0, 132.5, -720.3], 0.3, 1.0, Ground),
    p(UpperTerrace, CandyCane, [277.2, 132.5, -720.4], -0.2, 1.0, Ground),
    p(UpperTerrace, CandyCane, [278.6, 132.5, -716.8], -0.6, 1.0, Ground),
    p(UpperTerrace, Lantern, [268.0, 132.5, -715.9], 0.0, 1.0, Ground),
    p(UpperTerrace, Lantern, [274.0, 132.5, -715.9], 0.0, 1.0, Ground),
    p(UpperTerrace, Presents, [262.3, 132.5, -718.6], 0.3, 1.0, Ground),
    p(UpperTerrace, Presents, [280.2, 132.5, -712.0], -0.8, 1.0, Ground),
    // MainFloor.
    p(MainFloor, BigTree, [355.5, 107.0, -606.5], 0.0, 1.0, Top),
    p(MainFloor, SnowGlobe, [348.7, 105.6, -612.6], 0.79, 0.79, Top),
    // DeepPit.
    p(DeepPit, Presents, [222.5, 89.0, -701.5], 0.0, 0.6, Ground),
    p(DeepPit, Presents, [224.5, 89.0, -702.8], 0.7, 0.6, Ground),
    p(DeepPit, Presents, [226.8, 89.0, -702.0], 1.4, 0.6, Ground),
    p(DeepPit, Presents, [229.5, 89.0, -701.2], 2.1, 0.6, Ground),
    p(DeepPit, Presents, [231.8, 89.0, -702.4], 2.8, 0.6, Ground),
    p(DeepPit, Presents, [220.5, 89.0, -699.8], 3.5, 0.6, Ground),
    p(DeepPit, Presents, [233.0, 89.0, -700.0], 4.2, 0.6, Ground),
    p(DeepPit, Presents, [218.5, 89.0, -701.0], 4.9, 0.6, Ground),
    p(DeepPit, Reindeer, [222.0, 89.2, -696.0], 0.0, 1.3, Top),
    p(DeepPit, Reindeer, [216.0, 89.2, -697.5], 0.0, 1.3, Top),
    p(DeepPit, Uplight, [230.0, 89.0, -696.5], 0.0, 1.0, Top),
    p(DeepPit, Uplight, [240.0, 89.0, -696.5], 0.0, 1.0, Top),
    p(DeepPit, Uplight, [250.0, 89.0, -696.5], 0.0, 1.0, Top),
    // SouthRim.
    p(SouthRim, Snowman, [362.5, 139.5, -575.5], 3.14, 1.3, Top),
    p(SouthRim, Snowman, [362.5, 139.5, -568.5], 3.14, 1.3, Top),
    p(SouthRim, Snowman, [362.0, 140.5, -549.0], 3.14, 1.3, Top),
    p(SouthRim, Snowman, [360.0, 139.5, -572.0], 3.14, 0.75, Top),
    p(SouthRim, Snowman, [365.0, 139.5, -564.0], 3.14, 0.75, Top),
    p(SouthRim, Snowman, [360.5, 140.5, -553.0], 3.14, 0.75, Top),
    p(SouthRim, Snowman, [364.8, 139.5, -577.6], 3.14, 0.75, Top),
    p(SouthRim, Lantern, [358.6, 139.5, -581.0], 3.14, 1.0, Top),
    p(SouthRim, Lantern, [366.4, 139.5, -581.0], 3.14, 1.0, Top),
    p(SouthRim, Lantern, [358.6, 141.0, -546.0], 3.14, 1.0, Top),
    p(SouthRim, Lantern, [366.4, 141.0, -546.0], 3.14, 1.0, Top),
    // SouthGate.
    p(SouthGate, Uplight, [262.9, 77.5, -533.0], 1.57, 1.0, Top),
    p(SouthGate, Uplight, [288.6, 77.5, -533.0], -1.57, 1.0, Top),
    // TopDeck.
    p(TopDeck, Presents, [323.45, 140.6, -757.66], -0.89, 1.0, Ground),
    p(TopDeck, Uplight, [318.38, 141.0, -766.42], -0.89, 1.0, Top),
    p(TopDeck, Uplight, [322.15, 141.0, -761.75], -0.89, 1.0, Top),
    p(TopDeck, Uplight, [327.16, 141.0, -755.52], -0.89, 1.0, Top),
    p(TopDeck, Uplight, [332.18, 141.0, -749.28], -0.89, 1.0, Top),
    p(TopDeck, CandyCane, [315.18, 141.0, -767.69], -0.89, 1.5, Top),
    // WestGate.
    p(WestGate, SnowGlobe, [144.6, 160.3, -720.4], -0.37, 1.0, Ground),
    p(WestGate, Snowman, [151.0, 160.3, -720.8], -0.6, 0.75, Ground),
    p(WestGate, Lantern, [156.0, 160.3, -719.5], 0.0, 1.0, Ground),
    p(WestGate, Presents, [175.3, 160.3, -709.6], -0.37, 1.0, Ground),
    p(WestGate, SnowGlobe, [182.4, 160.3, -709.2], -0.37, 0.79, Ground),
    // UpperTerrace.
    p(UpperTerrace, Snowman, [264.6, 132.5, -706.4], 0.8, 0.75, Top),
    p(UpperTerrace, SnowGlobe, [282.6, 132.5, -700.3], -0.6, 0.79, Ground),
    p(UpperTerrace, Uplight, [265.0, 132.5, -722.0], 3.14, 1.0, Top),
    p(UpperTerrace, Uplight, [275.5, 132.5, -720.2], 3.14, 1.0, Top),
    p(UpperTerrace, Snowman, [258.6, 132.5, -719.6], 0.9, 0.75, Ground),
    p(UpperTerrace, Snowman, [277.8, 132.5, -706.0], -0.5, 0.75, Ground),
    p(UpperTerrace, Snowman, [281.6, 132.5, -705.0], -0.8, 1.15, Ground),
    p(UpperTerrace, Presents, [276.2, 132.5, -712.2], 0.0, 0.56, Ground),
    p(UpperTerrace, Presents, [264.8, 132.5, -712.0], 0.0, 0.56, Ground),
    // MainFloor.
    p(MainFloor, Presents, [370.5, 107.0, -603.5], 0.4, 0.9, Top),
    p(MainFloor, SnowGlobe, [376.5, 106.5, -612.0], -0.4, 0.79, Top),
    p(MainFloor, Reindeer, [389.0, 123.0, -604.0], -0.9, 1.3, Top),
    p(MainFloor, Snowman, [385.0, 122.5, -598.0], -0.7, 1.3, Top),
    p(MainFloor, Lantern, [352.41, 105.0, -613.01], 0.0, 1.0, Top),
    p(MainFloor, Snowman, [366.0, 107.0, -602.0], -0.6, 0.75, Top),
    p(MainFloor, Uplight, [343.4, 105.5, -609.6], -1.4, 1.0, Top),
    p(MainFloor, Uplight, [365.6, 107.5, -609.0], 1.57, 1.0, Top),
    p(MainFloor, Snowman, [381.0, 106.0, -614.0], -0.5, 0.75, Top),
    p(MainFloor, SnowGlobe, [394.0, 123.5, -604.0], -0.9, 0.79, Top),
    p(MainFloor, Lantern, [386.0, 122.5, -606.0], 0.0, 1.0, Top),
    // DeepPit.
    p(DeepPit, Sleigh, [209.5, 89.0, -585.0], 1.57, 1.0, Ground),
    p(DeepPit, Snowman, [208.0, 89.0, -591.5], 1.57, 0.75, Ground),
    p(DeepPit, Snowman, [211.0, 89.0, -578.5], 1.2, 0.75, Ground),
    p(DeepPit, Lantern, [207.2, 89.0, -596.4], 0.0, 1.0, Ground),
    p(DeepPit, Lantern, [207.2, 89.0, -573.6], 0.0, 1.0, Ground),
    p(DeepPit, Snowman, [211.8, 89.0, -592.0], 1.57, 1.15, Ground),
    p(DeepPit, Reindeer, [209.6, 89.0, -681.0], 1.2, 1.2, Top),
    p(DeepPit, Presents, [213.0, 89.0, -684.6], 0.8, 1.0, Top),
    p(DeepPit, Snowman, [226.0, 89.0, -697.4], 0.0, 0.75, Ground),
    p(DeepPit, Snowman, [237.5, 89.0, -694.8], 0.0, 0.75, Ground),
    p(DeepPit, Presents, [214.9, 89.0, -597.0], 0.0, 0.48, Ground),
    // SouthRim.
    p(SouthRim, Sleigh, [353.0, 139.0, -566.0], 3.14, 1.0, Top),
    p(SouthRim, Snowman, [354.5, 139.5, -558.0], 2.8, 0.75, Top),
    p(SouthRim, Snowman, [352.0, 139.5, -552.0], 3.4, 0.75, Top),
    p(SouthRim, Snowman, [352.0, 139.0, -573.0], 3.14, 1.15, Top),
    p(SouthRim, Lantern, [355.5, 140.0, -546.0], 0.0, 1.0, Top),
    p(SouthRim, Lantern, [355.5, 139.0, -578.5], 0.0, 1.0, Top),
    p(SouthRim, Snowman, [369.5, 140.0, -556.0], 3.0, 0.75, Top),
    p(SouthRim, Snowman, [370.5, 140.5, -548.0], 3.14, 1.15, Top),
    p(SouthRim, Uplight, [372.6, 140.5, -576.0], -1.57, 1.0, Top),
    p(SouthRim, Uplight, [373.6, 141.0, -563.0], -1.57, 1.0, Top),
    p(SouthRim, Reindeer, [379.5, 162.5, -568.0], 3.14, 1.3, Top),
    // SouthGate.
    p(SouthGate, Snowman, [257.5, 86.0, -521.0], 1.57, 1.15, Top),
    p(SouthGate, Snowman, [257.5, 86.0, -528.5], 1.57, 1.15, Top),
    p(SouthGate, Snowman, [252.5, 88.5, -517.0], 1.57, 0.75, Top),
    p(SouthGate, Uplight, [259.5, 83.0, -533.0], 0.0, 1.0, Top),
    p(SouthGate, Presents, [295.2, 79.5, -514.5], -1.2, 0.8, Top),
    p(SouthGate, SnowGlobe, [295.5, 80.0, -521.0], -1.57, 0.71, Top),
    p(SouthGate, Snowman, [294.5, 79.5, -525.5], -1.57, 1.15, Top),
    p(SouthGate, Uplight, [290.6, 80.5, -534.6], 0.0, 1.0, Top),
    p(SouthGate, Reindeer, [279.0, 108.5, -565.0], 0.0, 1.0, Top),
    p(SouthGate, Snowman, [276.0, 108.5, -558.0], 0.3, 1.15, Top),
    p(SouthGate, Snowman, [285.2, 110.5, -559.9], -0.3, 1.15, Top),
    p(SouthGate, SnowGlobe, [282.85, 110.5, -553.35], 0.0, 0.79, Top),
    p(SouthGate, Snowman, [276.0, 109.5, -556.0], 0.0, 0.75, Top),
    // TopDeck.
    p(TopDeck, Lantern, [344.0, 151.0, -735.1], 1.57, 1.6, Top),
    p(TopDeck, Lantern, [343.9, 151.0, -722.9], 1.57, 1.6, Top),
    p(TopDeck, Lantern, [293.0, 141.2, -783.4], 0.0, 1.0, Top),
    p(TopDeck, Gingerbread, [254.8, 143.6, -771.5], 1.57, 1.0, Top),
    p(TopDeck, Snowman, [341.2, 140.6, -742.3], 2.4, 0.75, Top),
    p(TopDeck, Snowman, [258.5, 143.0, -785.5], 1.2, 0.75, Top),
    p(TopDeck, Snowman, [252.0, 143.5, -777.5], 1.9, 0.75, Top),
    p(TopDeck, Lantern, [259.8, 142.5, -784.8], 0.0, 1.0, Top),
    p(TopDeck, Snowman, [250.0, 143.5, -782.5], 1.57, 1.15, Top),
    p(TopDeck, SnowGlobe, [259.6, 142.5, -772.0], 1.2, 0.79, Top),
    p(TopDeck, Reindeer, [358.6, 143.8, -741.2], 2.6, 1.3, Top),
    p(TopDeck, Snowman, [355.0, 143.8, -743.5], 2.8, 0.75, Top),
    p(TopDeck, SnowGlobe, [347.5, 143.0, -750.5], 2.4, 0.79, Top),
    p(TopDeck, Lantern, [362.5, 143.8, -741.0], 0.0, 1.0, Top),
    p(TopDeck, Presents, [351.5, 143.5, -745.0], 2.4, 1.0, Top),
    // WestGate.
    p(WestGate, Snowman, [156.83, 151.5, -728.34], 2.77, 1.15, Top),
    p(WestGate, Snowman, [180.16, 151.5, -719.31], 2.77, 1.15, Top),
    p(WestGate, Uplight, [158.78, 151.5, -728.12], 2.77, 1.0, Top),
    p(WestGate, Uplight, [178.56, 151.5, -720.46], 2.77, 1.0, Top),
    // UpperTerrace.
    p(UpperTerrace, Uplight, [262.5, 132.5, -722.9], 0.0, 1.0, Top),
    p(UpperTerrace, Uplight, [277.0, 132.5, -721.2], 0.0, 1.0, Top),
    p(UpperTerrace, Uplight, [287.0, 132.5, -712.9], 0.0, 1.0, Top),
    p(UpperTerrace, Presents, [260.4, 132.5, -722.6], 0.8, 0.6, Top),
    // MainFloor.
    p(MainFloor, Lantern, [377.8, 102.5, -620.2], 0.0, 1.0, Top),
    p(MainFloor, Lantern, [392.8, 101.5, -619.2], 0.0, 1.0, Top),
    p(MainFloor, Presents, [393.5, 106.0, -663.5], 0.4, 0.6, Top),
    p(MainFloor, Presents, [386.6, 106.0, -663.4], -0.6, 0.6, Top),
    // DeepPit.
    p(DeepPit, Snowman, [268.2, 89.0, -684.0], -1.57, 1.15, Ground),
    p(DeepPit, Snowman, [268.2, 89.0, -678.0], -1.57, 1.15, Ground),
    p(DeepPit, Lantern, [207.0, 89.0, -590.0], 0.0, 1.0, Top),
    p(DeepPit, Lantern, [207.0, 89.0, -578.0], 0.0, 1.0, Top),
    // SouthRim.
    p(SouthRim, Presents, [369.2, 139.5, -575.5], -1.57, 1.0, Top),
    p(SouthRim, Snowman, [355.0, 140.5, -543.0], 0.6, 1.0, Top),
    p(SouthRim, Presents, [357.4, 140.5, -542.2], 0.0, 0.8, Top),
    p(SouthRim, Snowman, [369.5, 140.0, -560.0], -1.57, 0.75, Top),
    p(SouthRim, Uplight, [361.0, 140.0, -558.6], 3.14, 1.0, Top),
    p(SouthRim, Uplight, [350.2, 139.5, -560.0], 0.0, 1.0, Top),
    // SouthGate.
    p(SouthGate, Snowman, [248.5, 90.5, -517.0], 1.57, 1.15, Top),
    p(SouthGate, Snowman, [250.0, 87.5, -507.5], 1.57, 1.15, Top),
    // WestGate.
    p(WestGate, Wreath, [161.56, 160.45, -705.91], -0.37, 1.0, Hung),
    // TopDeck.
    p(TopDeck, Wreath, [320.19, 148.6, -764.02], -0.89, 0.8, Hung),
    p(TopDeck, Presents, [275.0, 142.08, -777.0], 0.0, 0.55, Top),
    p(TopDeck, Presents, [280.55, 142.05, -777.0], 0.0, 0.55, Top),
    p(TopDeck, Presents, [285.35, 141.92, -776.8], 0.0, 0.55, Top),
    p(TopDeck, Presents, [290.45, 142.4, -776.2], 0.0, 0.55, Top),
    p(TopDeck, Wreath, [290.45, 141.25, -774.18], 0.0, 0.32, Hung),
    // SouthGate.
    p(SouthGate, Wreath, [275.75, 96.2, -535.95], 0.0, 1.0, Hung),
    // SouthRim.
    p(SouthRim, Sled, [364.0, 140.5, -556.5], 0.4, 1.0, Top),
    p(SouthRim, Sled, [361.0, 140.5, -563.5], -0.3, 1.0, Top),
];

const XM_ROWS: &[Row] = &[
    // WestGate.
    row(WestGate, Lantern, &[[152.53, 163.0, -710.85], [158.69, 163.0, -708.47]], 1.45, 1.0, Top),
    row(WestGate, Lantern, &[[165.41, 163.0, -705.87], [171.56, 163.0, -703.49]], 1.45, 1.0, Top),
    // TopDeck.
    row(TopDeck, CandyCane, &[[266.4, 141.6, -761.0], [266.4, 141.6, -784.1], [282.6, 141.2, -784.1]], 1.6, 1.0, Top),
    // UpperTerrace.
    row(UpperTerrace, CandyCane, &[
        [262.4, 132.5, -714.0], [262.4, 132.5, -709.5], [278.9, 132.5, -709.5],
    ], 1.5, 1.0, Ground),
    // MainFloor.
    row(MainFloor, Snowman, &[[360.0, 106.5, -616.0], [371.0, 106.5, -616.0]], 2.75, 1.3, Top),
    row(MainFloor, Snowman, &[[345.8, 106.0, -611.0], [345.8, 106.0, -603.0]], 4.0, 1.3, Top),
    row(MainFloor, Snowman, &[[349.5, 107.5, -600.5], [361.5, 107.5, -600.5]], 12.0, 1.3, Top),
    // DeepPit.
    row(DeepPit, Lantern, &[[206.52, 94.2, -672.1], [206.52, 94.2, -664.0]], 1.6, 0.75, Top),
    row(DeepPit, Lantern, &[[206.52, 94.2, -652.0], [206.52, 94.2, -644.0]], 1.6, 0.75, Top),
    // SouthRim.
    row(SouthRim, CandyCane, &[[358.6, 140.54, -579.5], [358.6, 141.46, -547.0]], 2.0, 1.0, Top),
    row(SouthRim, CandyCane, &[[366.4, 141.46, -547.0], [366.4, 140.54, -579.5]], 2.0, 1.0, Top),
    row(SouthRim, CandyCane, &[[364.9, 140.5, -581.0], [360.1, 140.5, -581.0]], 2.0, 1.0, Top),
    row(SouthRim, Lantern, &[[365.5, 139.0, -579.2], [359.5, 139.0, -579.2]], 1.5, 1.0, Top),
    // SouthGate.
    row(SouthGate, Lantern, &[[255.5, 88.0, -515.0], [255.5, 88.0, -529.5]], 2.4, 1.0, Top),
    row(SouthGate, Lantern, &[[291.2, 80.5, -530.8], [291.2, 80.5, -517.0]], 3.0, 1.0, Top),
    row(SouthGate, CandyCane, &[[254.0, 89.0, -515.0], [254.0, 89.0, -527.0]], 1.8, 1.0, Top),
    row(SouthGate, CandyCane, &[[292.8, 81.0, -532.0], [292.8, 81.0, -515.8]], 1.8, 1.0, Top),
    // TopDeck.
    row(TopDeck, CandyCane, &[[316.59, 140.6, -767.54], [322.91, 140.6, -759.92]], 1.5, 1.0, Ground),
    row(TopDeck, CandyCane, &[[325.54, 140.6, -756.65], [334.67, 140.6, -745.39]], 1.5, 1.0, Ground),
    // WestGate.
    row(WestGate, CandyCane, &[[147.0, 160.3, -715.5], [155.0, 160.3, -715.5]], 1.6, 1.0, Ground),
    row(WestGate, CandyCane, &[[177.2, 160.3, -703.6], [185.2, 160.3, -703.6]], 1.6, 1.0, Ground),
    // UpperTerrace.
    row(UpperTerrace, Lantern, &[
        [256.6, 132.5, -720.5], [257.6, 132.5, -712.8], [264.0, 132.5, -706.8], [270.6, 132.5, -701.4],
    ], 3.0, 1.0, Ground),
    row(UpperTerrace, CandyCane, &[[287.0, 132.5, -712.6], [305.0, 132.5, -712.6]], 1.6, 1.0, Ground),
    // MainFloor.
    row(MainFloor, Presents, &[[374.0, 108.5, -611.0], [374.0, 108.5, -605.0]], 1.5, 0.48, Top),
    // DeepPit.
    row(DeepPit, Lantern, &[[236.0, 89.0, -693.6], [244.0, 89.0, -693.6]], 2.0, 1.0, Ground),
    row(DeepPit, Lantern, &[[248.0, 89.0, -693.6], [254.0, 89.0, -693.6]], 2.0, 1.0, Ground),
    row(DeepPit, Lantern, &[[206.52, 94.2, -630.4], [206.52, 94.2, -622.4]], 1.6, 0.75, Top),
    row(DeepPit, Lantern, &[[206.52, 94.2, -610.9], [206.52, 94.2, -602.9]], 1.6, 0.75, Top),
    row(DeepPit, Lantern, &[[214.9, 89.0, -591.25], [214.9, 89.0, -574.0]], 2.88, 1.0, Ground),
    // SouthRim.
    row(SouthRim, Lantern, &[[356.6, 139.0, -578.0], [356.6, 140.0, -548.0]], 2.0, 1.0, Top),
    // SouthGate.
    row(SouthGate, Lantern, &[[267.0, 103.2, -537.6], [271.0, 103.2, -537.6]], 2.0, 1.0, Top),
    row(SouthGate, Lantern, &[[281.0, 103.2, -537.6], [285.0, 103.2, -537.6]], 2.0, 1.0, Top),
    // TopDeck.
    row(TopDeck, CandyCane, &[[256.2, 143.8, -784.0], [256.2, 143.6, -778.0]], 2.0, 1.0, Top),
    row(TopDeck, CandyCane, &[[253.4, 144.0, -784.0], [253.4, 143.8, -776.0]], 2.0, 1.0, Top),
    row(TopDeck, Lantern, &[[262.6, 143.0, -785.0], [262.6, 142.6, -765.0]], 4.0, 1.0, Top),
    row(TopDeck, CandyCane, &[[339.6, 140.6, -744.6], [341.5, 140.6, -744.0], [342.4, 140.6, -743.2]], 1.2, 1.0, Top),
    row(TopDeck, CandyCane, &[[264.3, 143.5, -771.5], [264.3, 143.5, -760.5]], 2.4, 1.0, Top),
    row(TopDeck, CandyCane, &[[268.0, 143.5, -784.95], [294.4, 143.5, -784.95]], 4.4, 1.0, Top),
    row(TopDeck, CandyCane, &[[250.0, 144.0, -780.0], [250.0, 144.0, -773.6]], 1.6, 1.0, Top),
    // WestGate.
    row(WestGate, CandyCane, &[[154.09, 151.5, -728.75], [155.2, 151.5, -729.93]], 1.3, 1.0, Top),
    row(WestGate, CandyCane, &[[182.46, 151.5, -717.78], [182.44, 151.5, -719.39]], 1.3, 1.0, Top),
    // MainFloor.
    row(MainFloor, Presents, &[[367.0, 104.5, -661.9], [393.0, 104.5, -661.9]], 2.2, 0.48, Top),
    // SouthGate.
    row(SouthGate, CandyCane, &[[249.3, 89.0, -514.5], [249.3, 87.8, -510.0]], 1.5, 1.0, Top),
];

const XM_ROUTES: &[Route] = &[
    // WestGate.
    swag(WestGate, Warm, true, &[
        [151.08, 159.55, -710.16], [152.95, 158.5, -709.44], [155.84, 158.5, -708.32], [158.73, 158.5, -707.2],
        [161.63, 158.5, -706.08], [164.52, 158.5, -704.96], [167.41, 158.5, -703.84], [170.3, 158.5, -702.72],
        [172.17, 159.55, -702.0],
    ]),
    swag(WestGate, Multi, false, &[
        [151.08, 162.35, -710.16], [154.6, 162.35, -708.8], [158.11, 162.35, -707.44], [161.63, 162.35, -706.08],
        [165.14, 162.35, -704.72], [168.65, 162.35, -703.36], [172.17, 162.35, -702.0],
    ]),
    // TopDeck.
    swag(TopDeck, Warm, true, &[
        [316.5, 139.2, -722.1], [324.3, 139.2, -722.1], [332.1, 139.2, -722.1], [339.9, 139.2, -722.1],
    ]),
    spiral(TopDeck, Multi, &[[344.0, 140.5, -735.1], [344.0, 151.0, -735.1]], 0.85, 0.85, 7.0),
    spiral(TopDeck, Multi, &[[343.9, 140.5, -722.9], [343.9, 151.0, -722.9]], 0.85, 0.85, 7.0),
    // UpperTerrace.
    taut(UpperTerrace, Multi, false, &[
        [260.8, 132.37, -709.0], [260.8, 132.37, -722.0], [280.6, 132.37, -720.6], [280.6, 132.37, -709.0],
    ]),
    taut(UpperTerrace, Multi, false, &[[271.0, 139.0, -722.25], [260.8, 132.37, -709.0]]),
    taut(UpperTerrace, Multi, false, &[[271.0, 139.0, -722.25], [280.6, 132.37, -709.0]]),
    // MainFloor.
    taut(MainFloor, Multi, false, &[
        [351.0, 122.0, -613.1], [354.63, 117.27, -607.11], [355.66, 117.27, -606.22], [364.5, 107.3, -602.5],
    ]),
    taut(MainFloor, Multi, false, &[
        [366.6, 112.0, -608.5], [355.7, 117.27, -606.62], [354.3, 117.27, -606.66], [346.5, 105.5, -608.5],
    ]),
    taut(MainFloor, Multi, false, &[
        [340.5, 124.0, -607.5], [354.29, 117.27, -606.55], [355.52, 117.27, -606.99], [363.5, 106.0, -614.5],
    ]),
    taut(MainFloor, Multi, false, &[
        [347.5, 106.7, -601.0], [354.42, 117.27, -606.08], [355.71, 117.27, -606.43], [365.0, 107.0, -605.5],
    ]),
    // DeepPit.
    swag(DeepPit, Warm, true, &[
        [205.9, 89.0, -688.0], [205.9, 89.0, -660.0], [205.9, 89.0, -632.0], [205.9, 89.0, -604.0],
        [205.9, 89.0, -576.0],
    ]),
    icicles(DeepPit, Ice, false, &[
        [207.15, 87.0, -672.0], [207.15, 87.0, -654.0], [207.15, 87.0, -636.0], [207.15, 87.0, -618.0],
        [207.15, 87.0, -600.0],
    ]),
    // SouthRim.
    swag(SouthRim, Multi, false, &[[349.15, 145.5, -558.8], [362.2, 145.5, -557.4]]),
    taut(SouthRim, Multi, false, &[
        [359.1, 138.28, -578.0], [359.1, 138.52, -574.0], [359.1, 138.74, -570.0], [359.1, 138.99, -566.0],
        [359.1, 139.24, -562.0], [359.1, 139.49, -558.0], [359.1, 139.74, -554.0], [359.1, 139.99, -550.0],
        [359.1, 140.24, -546.0],
    ]),
    taut(SouthRim, Multi, false, &[
        [365.9, 138.24, -578.0], [365.9, 138.46, -574.0], [365.9, 138.68, -570.0], [365.9, 138.89, -566.0],
        [365.9, 139.11, -562.0], [365.9, 139.33, -558.0], [365.9, 139.56, -554.0], [365.9, 139.78, -550.0],
        [365.9, 140.0, -546.0],
    ]),
    // SouthGate.
    swag(SouthGate, Warm, true, &[
        [266.0, 100.5, -536.2], [271.0, 100.5, -536.2], [276.0, 100.5, -536.2], [281.0, 100.5, -536.2],
        [286.0, 100.5, -536.2],
    ]),
    swag(SouthGate, Multi, false, &[
        [266.0, 92.0, -536.0], [271.0, 92.0, -536.0], [276.0, 92.0, -536.0], [281.0, 92.0, -536.0],
        [286.0, 92.0, -536.0],
    ]),
    // TopDeck.
    taut(TopDeck, Multi, false, &[
        [316.79, 146.6, -768.22], [323.21, 146.6, -760.55], [329.63, 146.6, -752.88], [336.04, 146.6, -745.21],
    ]),
    swag(TopDeck, Warm, true, &[
        [316.61, 146.3, -768.07], [319.82, 146.3, -764.24], [323.02, 146.3, -760.4], [326.23, 146.3, -756.56],
        [329.44, 146.3, -752.73], [332.65, 146.3, -748.89], [335.85, 146.3, -745.06],
    ]),
    taut(TopDeck, Multi, false, &[[316.88, 151.35, -768.03], [320.48, 151.35, -763.74], [324.08, 151.35, -759.46]]),
    swag(TopDeck, Warm, true, &[
        [316.1, 146.3, -784.6], [316.1, 146.3, -780.0], [316.1, 146.3, -775.4], [316.1, 146.3, -770.8],
    ]),
    taut(TopDeck, Multi, false, &[
        [273.94, 141.0, -777.3], [273.94, 141.0, -775.04], [275.0, 141.0, -775.04], [276.06, 141.0, -775.04],
        [276.06, 141.0, -777.3], [276.06, 141.0, -779.56], [275.0, 141.0, -779.56], [273.94, 141.0, -779.56],
        [273.94, 141.0, -777.3],
    ]),
    taut(TopDeck, Multi, false, &[
        [279.54, 141.0, -777.2], [279.54, 141.0, -775.04], [280.55, 141.0, -775.04], [281.56, 141.0, -775.04],
        [281.56, 141.0, -777.2], [281.56, 141.0, -779.36], [280.55, 141.0, -779.36], [279.54, 141.0, -779.36],
        [279.54, 141.0, -777.2],
    ]),
    taut(TopDeck, Multi, false, &[
        [284.34, 141.0, -776.95], [284.34, 141.0, -774.84], [285.35, 141.0, -774.84], [286.36, 141.0, -774.84],
        [286.36, 141.0, -776.95], [286.36, 141.0, -779.06], [285.35, 141.0, -779.06], [284.34, 141.0, -779.06],
        [284.34, 141.0, -776.95],
    ]),
    taut(TopDeck, Multi, false, &[
        [289.34, 141.0, -776.75], [289.34, 141.0, -774.24], [290.45, 141.0, -774.24], [291.56, 141.0, -774.24],
        [291.56, 141.0, -776.75], [291.56, 141.0, -779.26], [290.45, 141.0, -779.26], [289.34, 141.0, -779.26],
        [289.34, 141.0, -776.75],
    ]),
    swag(TopDeck, Warm, true, &[
        [266.0, 142.6, -784.35], [274.0, 142.6, -784.35], [282.0, 142.6, -784.35], [290.0, 142.6, -784.35],
        [298.0, 142.6, -784.35], [306.0, 142.6, -784.35], [314.0, 142.6, -784.35],
    ]),
    swag(TopDeck, Warm, true, &[
        [265.24, 142.5, -783.8], [265.04, 142.5, -776.2], [264.83, 142.5, -768.6], [264.63, 142.5, -761.0],
    ]),
    swag(TopDeck, Warm, true, &[
        [316.69, 150.9, -767.88], [318.49, 150.9, -765.74], [320.29, 150.9, -763.59], [322.09, 150.9, -761.45],
        [323.9, 150.9, -759.31],
    ]),
    icicles(TopDeck, Ice, false, &[[316.37, 146.6, -784.6], [316.37, 146.6, -777.7], [316.37, 146.6, -770.8]]),
    icicles(TopDeck, Ice, false, &[
        [316.5, 139.95, -722.27], [322.35, 139.95, -722.27], [328.2, 139.95, -722.27], [334.05, 139.95, -722.27],
        [339.9, 139.95, -722.27],
    ]),
    // WestGate.
    icicles(WestGate, Ice, false, &[
        [152.57, 162.35, -711.77], [155.83, 162.35, -710.51], [159.1, 162.35, -709.24], [162.36, 162.35, -707.98],
        [165.63, 162.35, -706.72], [168.89, 162.35, -705.45], [172.16, 162.35, -704.19],
    ]),
    icicles(WestGate, Ice, false, &[
        [158.94, 158.05, -713.68], [161.38, 158.05, -712.74], [163.84, 158.05, -711.79], [166.29, 158.05, -710.84],
        [168.73, 158.05, -709.89],
    ]),
    // UpperTerrace.
    swag(UpperTerrace, Warm, true, &[
        [258.4, 139.6, -725.28], [264.0, 139.6, -723.67], [270.0, 139.6, -722.38], [276.0, 139.6, -721.63],
        [281.6, 139.6, -721.31],
    ]),
    // MainFloor.
    spiral(MainFloor, Multi, &[[350.55, 105.3, -613.08], [350.55, 123.8, -613.08]], 0.5, 0.5, 12.0),
    // SouthGate.
    icicles(SouthGate, Ice, false, &[[266.3, 102.45, -536.3], [269.05, 102.45, -536.3], [271.8, 102.45, -536.3]]),
    icicles(SouthGate, Ice, false, &[[280.2, 102.45, -536.3], [282.6, 102.45, -536.3], [285.0, 102.45, -536.3]]),
    // UpperTerrace.
    swag(UpperTerrace, Warm, true, &[[284.6, 140.6, -713.2], [289.3, 140.6, -713.2], [294.0, 140.6, -713.2]]),
    // DeepPit.
    icicles(DeepPit, Ice, false, &[
        [222.0, 88.8, -692.9], [231.0, 88.8, -692.9], [240.0, 88.8, -692.9], [249.0, 88.8, -692.9],
        [258.0, 88.8, -692.9],
    ]),
    // WestGate.
    swag(WestGate, Warm, true, &[
        [159.6, 158.6, -720.24], [162.86, 158.6, -718.98], [166.13, 158.6, -717.71], [169.39, 158.6, -716.45],
        [172.66, 158.6, -715.19],
    ]),
    // SouthGate.
    swag(SouthGate, Warm, true, &[
        [266.0, 100.5, -539.5], [271.0, 100.5, -539.5], [276.0, 100.5, -539.5], [281.0, 100.5, -539.5],
        [286.0, 100.5, -539.5],
    ]),
    swag(SouthGate, Multi, false, &[
        [266.0, 92.0, -539.5], [271.0, 92.0, -539.5], [276.0, 92.0, -539.5], [281.0, 92.0, -539.5],
        [286.0, 92.0, -539.5],
    ]),
    icicles(SouthGate, Ice, false, &[
        [263.2, 88.2, -536.4], [269.6, 88.2, -536.4], [276.0, 88.2, -536.4], [282.4, 88.2, -536.4],
        [288.6, 88.2, -536.4],
    ]),
];

const XM_WEBS: &[WebSpan] = &[
];

const XM_FLOCKS: &[[f32; 3]] = &[
];

const XM_ROOSTS: &[[f32; 3]] = &[
];

pub(crate) static CHRISTMAS: Layout<'static> = Layout {
    pieces: XM_PIECES,
    routes: XM_ROUTES,
    rows: XM_ROWS,
    webs: XM_WEBS,
    flocks: XM_FLOCKS,
    roosts: XM_ROOSTS,
};

pub(crate) fn layout(season: Season) -> &'static Layout<'static> {
    match season {
        Season::Halloween => &HALLOWEEN,
        Season::Christmas => &CHRISTMAS,
    }
}
